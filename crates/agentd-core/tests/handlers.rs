//! Tests for `agentd_core::handler` — the `Handler` trait, registry, and the
//! synchronous `conditional` + `tool` handlers. Names match the spec `Test:`
//! selectors. Requires the `test-support` feature (self dev-dependency).

use agentd_core::dot::parser;
use agentd_core::engine::HandlerStep;
use agentd_core::graph::{HandlerKind, NodeGraph};
use agentd_core::handler::{
    ConditionalHandler, Handler, HandlerCtx, HandlerRegistry, Ports, ToolHandler,
};
use agentd_core::ports::CommandOutput;
use agentd_core::test_support::{
    FakeBackend, FixedClock, InMemoryStore, MempalStub, RecordingCommandRunner,
};
use agentd_core::types::{Outcome, RunContext, RunId, Status};

/// Owns the five fakes + a run id so a `HandlerCtx` can borrow them for the
/// test's duration (avoids dangling borrows of short-lived temporaries).
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

fn ctx_with(key: &str, value: &str) -> RunContext {
    let mut c = RunContext::new();
    c.set(key, serde_json::Value::String(value.to_string()));
    c
}

/// A conditional node `branch` with two kv-conditioned, labelled edges.
fn two_branch_graph() -> NodeGraph {
    graph(
        r#"digraph m {
            "branch" [handler=conditional];
            "high" [handler=tool];
            "low" [handler=tool];
            "branch" -> "high" [condition="kv(\"score\")==\"high\"", label="go_high"];
            "branch" -> "low"  [condition="kv(\"score\")==\"low\"",  label="go_low"];
        }"#,
    )
}

fn done(step: HandlerStep) -> Outcome {
    match step {
        HandlerStep::Done(o) => o,
        HandlerStep::Park(r) => panic!("expected Done, parked with {r:?}"),
    }
}

#[tokio::test]
async fn handler_trait_is_object_safe_in_registry() {
    let reg = HandlerRegistry::with_builtins();
    let handler = reg
        .get(HandlerKind::Conditional)
        .expect("conditional registered");
    let g = two_branch_graph();
    let node = g.node("branch").expect("branch node");
    let context = ctx_with("score", "high");
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let step = handler
        .run(&mut ctx)
        .await
        .expect("run via Arc<dyn Handler>");
    assert!(matches!(step, HandlerStep::Done(_)));
}

#[tokio::test]
async fn conditional_picks_first_matching_branch() {
    let g = two_branch_graph();
    let node = g.node("branch").expect("branch node");
    let context = ctx_with("score", "high");
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let outcome = done(ConditionalHandler.run(&mut ctx).await.expect("run"));
    assert_eq!(outcome.status, Status::Success);
    assert_eq!(outcome.preferred_label.as_deref(), Some("go_high"));
}

#[tokio::test]
async fn conditional_returns_fail_when_no_branch_matches_and_no_default() {
    let g = two_branch_graph();
    let node = g.node("branch").expect("branch node");
    let context = ctx_with("score", "mid"); // matches neither high nor low
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let outcome = done(ConditionalHandler.run(&mut ctx).await.expect("run"));
    assert_eq!(outcome.status, Status::Fail);
    assert_eq!(outcome.preferred_label, None);
}

#[tokio::test]
async fn conditional_uses_default_branch_when_present() {
    let g = graph(
        r#"digraph m {
            "branch" [handler=conditional];
            "high" [handler=tool];
            "fallback" [handler=tool];
            "branch" -> "high"     [condition="kv(\"score\")==\"high\"", label="go_high"];
            "branch" -> "fallback" [label="go_default"];
        }"#,
    );
    let node = g.node("branch").expect("branch node");
    let context = ctx_with("score", "mid"); // the conditioned edge does not match
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let outcome = done(ConditionalHandler.run(&mut ctx).await.expect("run"));
    assert_eq!(outcome.status, Status::Success);
    assert_eq!(outcome.preferred_label.as_deref(), Some("go_default"));
}

fn tool_graph(extra_attrs: &str) -> NodeGraph {
    graph(&format!(
        r#"digraph m {{
            "run_it" [handler=tool, cmd="echo hi"{extra_attrs}];
        }}"#
    ))
}

#[tokio::test]
async fn tool_handler_captures_stdout_as_artifact_when_path_set() {
    let g = tool_graph(r#", artifact_path="out.txt""#);
    let node = g.node("run_it").expect("tool node");
    let context = RunContext::new();
    let deps = Deps::new();
    deps.runner.push_output(Ok(CommandOutput {
        stdout: "hello world".to_string(),
        stderr: String::new(),
        status: 0,
    }));
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let outcome = done(ToolHandler.run(&mut ctx).await.expect("run"));
    assert_eq!(outcome.status, Status::Success);
    assert_eq!(outcome.artifacts.len(), 1, "one artifact pointer");
    let art = &outcome.artifacts[0];
    assert_eq!(art.path, std::path::PathBuf::from("out.txt"));
    assert_eq!(art.bytes, "hello world".len() as u64);
    assert_eq!(art.sha256.len(), 64, "hex sha256 is 64 chars");
    // The recorded argv proves cmd was split into program + args.
    let calls = deps.runner.calls();
    assert_eq!(calls[0].program, "echo");
    assert_eq!(calls[0].args, vec!["hi".to_string()]);
}

#[tokio::test]
async fn tool_handler_maps_nonzero_exit_to_fail() {
    let g = tool_graph("");
    let node = g.node("run_it").expect("tool node");
    let context = RunContext::new();
    let deps = Deps::new();
    deps.runner.push_output(Ok(CommandOutput {
        stdout: String::new(),
        stderr: "boom".to_string(),
        status: 2,
    }));
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let outcome = done(ToolHandler.run(&mut ctx).await.expect("run"));
    assert_eq!(outcome.status, Status::Fail);
    assert!(outcome.artifacts.is_empty(), "no artifact on failure");
}

#[tokio::test]
async fn tool_handler_maps_command_error_to_retry() {
    let g = tool_graph("");
    let node = g.node("run_it").expect("tool node");
    let context = RunContext::new();
    let deps = Deps::new();
    deps.runner
        .push_output(Err(agentd_core::ports::CommandError {
            message: "timed out".to_string(),
            stderr: String::new(),
            status: None,
        }));
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let outcome = done(ToolHandler.run(&mut ctx).await.expect("run"));
    assert_eq!(outcome.status, Status::Retry);
}
