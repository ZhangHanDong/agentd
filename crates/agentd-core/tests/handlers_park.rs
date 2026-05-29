//! Tests for the park-style handlers (`wait.human`/`fan_out`/`fan_in`/
//! `codergen`). Names match the spec `Test:` selectors. Requires `test-support`.

use agentd_core::dot::parser;
use agentd_core::engine::{EngineEvent, HandlerStep, ParkReason};
use agentd_core::graph::NodeGraph;
use agentd_core::handler::{
    CodergenHandler, FanInHandler, FanOutHandler, Handler, HandlerCtx, Ports, WaitHumanHandler,
};
use agentd_core::ports::{DrawerHit, Store};
use agentd_core::test_support::{
    FakeBackend, FixedClock, InMemoryStore, MempalStub, RecordingCommandRunner,
};
use agentd_core::types::{
    AgentId, NodeId, Outcome, ReviewVerdict, RunContext, RunId, Status, VerdictValue,
};

struct Deps {
    run_id: RunId,
    backend: FakeBackend,
    runner: RecordingCommandRunner,
    store: InMemoryStore,
    mempal: MempalStub,
    clock: FixedClock,
}

impl Deps {
    fn new() -> Self {
        Self {
            run_id: RunId::from_string("r"),
            backend: FakeBackend::new(),
            runner: RecordingCommandRunner::new(),
            store: InMemoryStore::new(),
            mempal: MempalStub::new(),
            clock: FixedClock::new(0),
        }
    }

    fn ports(&self) -> Ports<'_> {
        Ports {
            backend: &self.backend,
            runner: &self.runner,
            store: &self.store,
            mempal: &self.mempal,
            clock: &self.clock,
        }
    }
}

fn graph(src: &str) -> NodeGraph {
    let ast = parser::parse(src).expect("dot parse");
    NodeGraph::from_ast_unvalidated(&ast)
}

fn done(step: HandlerStep) -> Outcome {
    match step {
        HandlerStep::Done(o) => o,
        HandlerStep::Park(r) => panic!("expected Done, parked with {r:?}"),
    }
}

// ---- wait.human -----------------------------------------------------------

fn wait_graph() -> NodeGraph {
    graph(
        r#"digraph m {
            "ask" [handler="wait.human", prompt="approve?"];
        }"#,
    )
}

#[tokio::test]
async fn wait_human_run_parks_with_human_answer_reason() {
    let g = wait_graph();
    let node = g.node("ask").expect("ask node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let step = WaitHumanHandler.run(&mut ctx).await.expect("run");
    let wait_id = match step {
        HandlerStep::Park(ParkReason::HumanAnswer { wait_id }) => wait_id,
        other => panic!("expected HumanAnswer park, got {other:?}"),
    };
    // The wait is open in the store and resolves back to (run, node).
    let parked = deps
        .store
        .lookup_park_by_wait_id(&wait_id)
        .await
        .expect("lookup");
    assert_eq!(
        parked,
        Some((RunId::from_string("r"), NodeId::parsed("ask")))
    );
}

#[tokio::test]
async fn wait_human_resume_stages_answer_and_returns_done() {
    let g = wait_graph();
    let node = g.node("ask").expect("ask node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let wait_id = match WaitHumanHandler.run(&mut ctx).await.expect("run") {
        HandlerStep::Park(ParkReason::HumanAnswer { wait_id }) => wait_id,
        other => panic!("expected park, got {other:?}"),
    };
    let step = WaitHumanHandler
        .resume(
            &mut ctx,
            EngineEvent::HumanAnswered {
                wait_id,
                answer: "approve".to_string(),
                feedback: None,
            },
        )
        .await
        .expect("resume");
    assert!(matches!(step, HandlerStep::Done(_)));
    assert_eq!(
        ctx.staged_updates()
            .get("answer")
            .and_then(serde_json::Value::as_str),
        Some("approve")
    );
}

#[tokio::test]
async fn wait_human_resume_sets_preferred_label_to_answer() {
    let g = wait_graph();
    let node = g.node("ask").expect("ask node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let wait_id = match WaitHumanHandler.run(&mut ctx).await.expect("run") {
        HandlerStep::Park(ParkReason::HumanAnswer { wait_id }) => wait_id,
        other => panic!("expected park, got {other:?}"),
    };
    let outcome = done(
        WaitHumanHandler
            .resume(
                &mut ctx,
                EngineEvent::HumanAnswered {
                    wait_id,
                    answer: "approve".to_string(),
                    feedback: Some("looks good".to_string()),
                },
            )
            .await
            .expect("resume"),
    );
    assert_eq!(outcome.preferred_label.as_deref(), Some("approve"));
}

// ---- fan_out --------------------------------------------------------------

fn fan_out_graph() -> NodeGraph {
    graph(
        r#"digraph m {
            "review" [handler="parallel.fan_out", reviewers="claude-sec,codex-perf,gemini-readability"];
        }"#,
    )
}

#[tokio::test]
async fn fan_out_run_parks_with_expected_reviewer_count() {
    let g = fan_out_graph();
    let node = g.node("review").expect("review node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let step = FanOutHandler.run(&mut ctx).await.expect("run");
    match step {
        HandlerStep::Park(ParkReason::ReviewVerdicts { expected, .. }) => assert_eq!(expected, 3),
        other => panic!("expected ReviewVerdicts park, got {other:?}"),
    }
    assert_eq!(deps.backend.spawned().len(), 3, "three reviewers spawned");
}

#[tokio::test]
async fn fan_out_computes_deterministic_context_sha_in_memory() {
    let g = fan_out_graph();
    let node = g.node("review").expect("review node");
    let mut context = RunContext::new();
    context.set("k", serde_json::Value::String("v".to_string()));

    let sha = |deps: &Deps, ctx: &HandlerCtx<'_>| {
        ctx.staged_updates()
            .get("context_sha")
            .and_then(serde_json::Value::as_str)
            .map_or_else(
                || {
                    panic!(
                        "no context_sha staged (deps spawned {})",
                        deps.backend.spawned().len()
                    )
                },
                ToString::to_string,
            )
    };

    let deps1 = Deps::new();
    let mut ctx1 = HandlerCtx::new(&deps1.run_id, &g, node, &context, deps1.ports());
    FanOutHandler.run(&mut ctx1).await.expect("run 1");
    let sha1 = sha(&deps1, &ctx1);

    let deps2 = Deps::new();
    let mut ctx2 = HandlerCtx::new(&deps2.run_id, &g, node, &context, deps2.ports());
    FanOutHandler.run(&mut ctx2).await.expect("run 2");
    let sha2 = sha(&deps2, &ctx2);

    assert_eq!(sha1, sha2, "context_sha must be deterministic");
    assert_eq!(sha1.len(), 64, "hex sha256 is 64 chars");
}

#[tokio::test]
async fn fan_out_resume_stays_parked_until_all_verdicts_in() {
    let g = fan_out_graph();
    let node = g.node("review").expect("review node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let review_run_id = match FanOutHandler.run(&mut ctx).await.expect("run") {
        HandlerStep::Park(ParkReason::ReviewVerdicts { review_run_id, .. }) => review_run_id,
        other => panic!("expected park, got {other:?}"),
    };

    let reviewers = ["claude-sec", "codex-perf", "gemini-readability"];
    for (i, reviewer) in reviewers.iter().enumerate() {
        let step = FanOutHandler
            .resume(
                &mut ctx,
                EngineEvent::ReviewVerdictSubmitted {
                    review_run_id: review_run_id.clone(),
                    reviewer_id: AgentId::parsed(*reviewer),
                    verdict: VerdictValue::Pass,
                },
            )
            .await
            .expect("resume");
        if i < 2 {
            assert!(
                matches!(step, HandlerStep::Park(_)),
                "verdict {i} should re-park"
            );
        } else {
            assert!(
                matches!(step, HandlerStep::Done(_)),
                "third verdict completes"
            );
        }
    }
}

// ---- fan_in ---------------------------------------------------------------

fn fan_in_graph(aggregator: &str) -> NodeGraph {
    graph(&format!(
        r#"digraph m {{
            "aggregate" [handler="parallel.fan_in", aggregator="{aggregator}"];
        }}"#
    ))
}

async fn seed_review(deps: &Deps, verdicts: &[(&str, VerdictValue)]) -> RunContext {
    let rr = deps
        .store
        .insert_review_run(
            &deps.run_id,
            &NodeId::parsed("review"),
            verdicts.len(),
            "sha",
        )
        .await
        .expect("insert review run");
    for (who, value) in verdicts {
        deps.store
            .insert_review_verdict(
                &rr,
                ReviewVerdict {
                    reviewer_id: AgentId::parsed(*who),
                    value: *value,
                },
            )
            .await
            .expect("insert verdict");
    }
    let mut context = RunContext::new();
    context.set(
        "review_run_id",
        serde_json::Value::String(rr.as_str().to_string()),
    );
    context
}

#[tokio::test]
async fn fan_in_aggregator_majority_pass_returns_success_when_majority() {
    let deps = Deps::new();
    let context = seed_review(
        &deps,
        &[
            ("a", VerdictValue::Pass),
            ("b", VerdictValue::Pass),
            ("c", VerdictValue::Fail),
        ],
    )
    .await;
    let g = fan_in_graph("majority_pass");
    let node = g.node("aggregate").expect("aggregate node");
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let outcome = done(FanInHandler.run(&mut ctx).await.expect("run"));
    assert_eq!(outcome.status, Status::Success);
}

#[tokio::test]
async fn fan_in_aggregator_any_fail_returns_fail_when_one_blocker() {
    let deps = Deps::new();
    let context = seed_review(
        &deps,
        &[
            ("a", VerdictValue::Pass),
            ("b", VerdictValue::Pass),
            ("c", VerdictValue::Block),
        ],
    )
    .await;
    let g = fan_in_graph("any_fail");
    let node = g.node("aggregate").expect("aggregate node");
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let outcome = done(FanInHandler.run(&mut ctx).await.expect("run"));
    assert_eq!(outcome.status, Status::Fail);
}

// ---- codergen -------------------------------------------------------------

fn codergen_graph() -> NodeGraph {
    graph(
        r#"digraph m {
            "implement" [handler="codergen", role="implementer", initial_prompt_includes="$spec_path", pre_tools="mempal_search"];
        }"#,
    )
}

#[tokio::test]
async fn codergen_run_parks_with_agent_outcome_reason_and_assembles_prompt() {
    let g = codergen_graph();
    let node = g.node("implement").expect("implement node");
    let mut context = RunContext::new();
    context.set(
        "spec_path",
        serde_json::Value::String("specs/x.spec.md".to_string()),
    );
    let deps = Deps::new();
    deps.mempal.set_hits(vec![DrawerHit {
        drawer_id: "d1".to_string(),
        body: "prior art note".to_string(),
        score: 0.9,
    }]);
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let step = CodergenHandler.run(&mut ctx).await.expect("run");
    assert!(
        matches!(step, HandlerStep::Park(ParkReason::AgentOutcome { .. })),
        "codergen parks awaiting agent outcome, got {step:?}"
    );
    let spawned = deps.backend.spawned();
    assert_eq!(spawned.len(), 1, "one implementer spawned");
    let prompt = spawned[0]
        .initial_prompt
        .as_deref()
        .expect("initial prompt set");
    assert!(
        prompt.contains("specs/x.spec.md"),
        "prompt carries spec_path: {prompt}"
    );
    assert!(
        prompt.contains("prior art note"),
        "prompt carries mempal hit: {prompt}"
    );
}

#[tokio::test]
async fn codergen_resume_returns_agent_reported_outcome() {
    let g = codergen_graph();
    let node = g.node("implement").expect("implement node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let task_run_id = match CodergenHandler.run(&mut ctx).await.expect("run") {
        HandlerStep::Park(ParkReason::AgentOutcome { task_run_id }) => task_run_id,
        other => panic!("expected park, got {other:?}"),
    };
    let outcome = done(
        CodergenHandler
            .resume(
                &mut ctx,
                EngineEvent::AgentOutcomeSubmitted {
                    task_run_id,
                    outcome: Outcome::fail(),
                },
            )
            .await
            .expect("resume"),
    );
    assert_eq!(outcome.status, Status::Fail);
}
