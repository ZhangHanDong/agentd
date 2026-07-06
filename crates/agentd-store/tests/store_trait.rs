//! Behavioral-parity tests: `SqliteStore` must behave like the P0.1
//! `InMemoryStore` it replaces. Two layers — (1) the replay/idempotency
//! invariants exercised directly through the `Store` trait, and (2) the real
//! engine driving the full canonical park/resume flow against `SqliteStore`.

use std::path::PathBuf;

use agentd_core::dot::parser;
use agentd_core::engine::{Engine, EngineEvent, ParkReason, RunProgress};
use agentd_core::graph::NodeGraph;
use agentd_core::handler::{HandlerRegistry, Ports};
use agentd_core::ports::{RunStatus, Store};
use agentd_core::test_support::{FakeBackend, FixedClock, MempalStub, RecordingCommandRunner};
use agentd_core::types::{
    AgentId, Artifact, ArtifactKind, NodeId, Outcome, ReviewVerdict, RunId, VerdictValue,
};
use agentd_store::SqliteStore;

async fn store() -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let s = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    (s, dir)
}

#[tokio::test]
async fn human_wait_answer_once_then_conflict_parity() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r");
    let node = NodeId::parsed("ask");
    s.insert_run(&run, "sha").await.expect("run");
    let wait = s.open_human_wait(&run, &node, "?").await.expect("open");
    assert_eq!(
        s.lookup_park_by_wait_id(&wait).await.expect("lookup"),
        Some((run.clone(), node.clone())),
        "open wait parks"
    );
    s.answer_human_wait(&wait, "approve", None)
        .await
        .expect("answer");
    assert!(
        s.lookup_park_by_wait_id(&wait)
            .await
            .expect("lookup")
            .is_none(),
        "answered wait no longer parks"
    );
    assert!(
        s.answer_human_wait(&wait, "approve", None).await.is_err(),
        "second answer must conflict"
    );
}

#[tokio::test]
async fn review_verdict_dedup_and_open_closed_parity() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r");
    s.insert_run(&run, "sha").await.expect("run");
    let rr = s
        .insert_review_run(&run, &NodeId::parsed("review"), 3, 1, "csha")
        .await
        .expect("review run");
    assert_eq!(s.review_expected(&rr).await.expect("expected"), Some(3));
    assert!(
        s.lookup_park_by_review_run(&rr)
            .await
            .expect("lookup")
            .is_some(),
        "open with 0/3 verdicts"
    );
    let vote = |who: &str| ReviewVerdict {
        reviewer_id: AgentId::parsed(who),
        value: VerdictValue::Pass,
        findings: String::new(),
    };
    s.insert_review_verdict(&rr, vote("a")).await.expect("a");
    s.insert_review_verdict(&rr, vote("a"))
        .await
        .expect("a dup is a no-op");
    assert_eq!(
        s.count_verdicts(&rr).await.expect("count"),
        1,
        "duplicate reviewer not double-counted"
    );
    assert!(
        s.lookup_park_by_review_run(&rr)
            .await
            .expect("lookup")
            .is_some(),
        "still open at 1/3"
    );
    s.insert_review_verdict(&rr, vote("b")).await.expect("b");
    s.insert_review_verdict(&rr, vote("c")).await.expect("c");
    assert_eq!(s.count_verdicts(&rr).await.expect("count"), 3);
    assert!(
        s.lookup_park_by_review_run(&rr)
            .await
            .expect("lookup")
            .is_none(),
        "closed at 3/3"
    );
    assert_eq!(s.list_verdicts(&rr).await.expect("list").len(), 3);
}

#[tokio::test]
async fn review_verdict_findings_round_trip_first_wins() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r");
    s.insert_run(&run, "sha").await.expect("run");
    let rr = s
        .insert_review_run(&run, &NodeId::parsed("review"), 1, 1, "csha")
        .await
        .expect("review run");

    s.insert_review_verdict(
        &rr,
        ReviewVerdict {
            reviewer_id: AgentId::parsed("claude-sec"),
            value: VerdictValue::Fail,
            findings: "first finding".to_string(),
        },
    )
    .await
    .expect("first verdict");
    s.insert_review_verdict(
        &rr,
        ReviewVerdict {
            reviewer_id: AgentId::parsed("claude-sec"),
            value: VerdictValue::Pass,
            findings: "second finding must not overwrite".to_string(),
        },
    )
    .await
    .expect("duplicate verdict is a no-op");

    let verdicts = s.list_verdicts(&rr).await.expect("list");
    assert_eq!(verdicts.len(), 1, "duplicate reviewer is still first-wins");
    assert_eq!(verdicts[0].value, VerdictValue::Fail);
    assert_eq!(verdicts[0].findings, "first finding");
}

#[tokio::test]
async fn reviewer_worktree_mapping_is_take_once() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r");
    s.insert_run(&run, "sha").await.expect("run");
    let rr = s
        .insert_review_run(&run, &NodeId::parsed("review"), 1, 1, "csha")
        .await
        .expect("review run");
    let reviewer = AgentId::parsed("claude-sec");
    let path = PathBuf::from("/tmp/review-claude-sec");

    s.set_review_worktree(&rr, &reviewer, &path)
        .await
        .expect("set reviewer worktree");

    assert_eq!(
        s.take_review_worktree(&rr, &reviewer)
            .await
            .expect("take first"),
        Some(path),
        "first take returns the reviewer worktree"
    );
    assert_eq!(
        s.take_review_worktree(&rr, &reviewer)
            .await
            .expect("take second"),
        None,
        "second take is empty so replayed verdicts cannot release twice"
    );
}

#[tokio::test]
async fn active_worktree_paths_include_non_finished_task_and_review_worktrees() {
    let (s, _d) = store().await;

    let running = RunId::from_string("running");
    s.insert_run(&running, "sha").await.expect("running run");
    let running_task = s
        .insert_task_run(&running, &NodeId::parsed("implement"))
        .await
        .expect("running task");
    s.set_task_run_worktree(
        &running_task,
        &PathBuf::from("/tmp/wt-task-tr_0123456789ABCDEFGHJKMNPQRS"),
    )
    .await
    .expect("running task worktree");
    s.complete_task_run(&running_task)
        .await
        .expect("task may be complete before workflow terminal");
    let running_review = s
        .insert_review_run(&running, &NodeId::parsed("review"), 2, 1, "csha")
        .await
        .expect("running review");
    let released_reviewer = AgentId::parsed("released");
    s.set_review_worktree(
        &running_review,
        &released_reviewer,
        &PathBuf::from("/tmp/wt-review-rr_0123456789ABCDEFGHJKMNPQRS-released"),
    )
    .await
    .expect("set released reviewer worktree");
    assert!(
        s.take_review_worktree(&running_review, &released_reviewer)
            .await
            .expect("release reviewer")
            .is_some(),
        "released reviewer path was present before take"
    );
    s.set_review_worktree(
        &running_review,
        &AgentId::parsed("active"),
        &PathBuf::from("/tmp/wt-review-rr_0123456789ABCDEFGHJKMNPQRS-active"),
    )
    .await
    .expect("set active reviewer worktree");

    let failed = RunId::from_string("failed");
    s.insert_run(&failed, "sha").await.expect("failed run");
    let failed_task = s
        .insert_task_run(&failed, &NodeId::parsed("implement"))
        .await
        .expect("failed task");
    s.set_task_run_worktree(
        &failed_task,
        &PathBuf::from("/tmp/wt-task-tr_11111111111111111111111111"),
    )
    .await
    .expect("failed task worktree");
    let failed_review = s
        .insert_review_run(&failed, &NodeId::parsed("review"), 1, 1, "csha")
        .await
        .expect("failed review");
    s.set_review_worktree(
        &failed_review,
        &AgentId::parsed("debug"),
        &PathBuf::from("/tmp/wt-review-rr_11111111111111111111111111-debug"),
    )
    .await
    .expect("failed reviewer worktree");
    s.update_run_status(&failed, RunStatus::Failed)
        .await
        .expect("mark failed");

    let finished = RunId::from_string("finished");
    s.insert_run(&finished, "sha").await.expect("finished run");
    let finished_task = s
        .insert_task_run(&finished, &NodeId::parsed("implement"))
        .await
        .expect("finished task");
    s.set_task_run_worktree(
        &finished_task,
        &PathBuf::from("/tmp/wt-task-tr_22222222222222222222222222"),
    )
    .await
    .expect("finished task worktree");
    let finished_review = s
        .insert_review_run(&finished, &NodeId::parsed("review"), 1, 1, "csha")
        .await
        .expect("finished review");
    s.set_review_worktree(
        &finished_review,
        &AgentId::parsed("done"),
        &PathBuf::from("/tmp/wt-review-rr_22222222222222222222222222-done"),
    )
    .await
    .expect("finished reviewer worktree");
    s.update_run_status(&finished, RunStatus::Finished)
        .await
        .expect("mark finished");

    let paths = s.active_worktree_paths().await.expect("active paths");
    let paths: std::collections::HashSet<PathBuf> = paths.into_iter().collect();

    assert!(
        paths.contains(&PathBuf::from("/tmp/wt-task-tr_0123456789ABCDEFGHJKMNPQRS")),
        "running workflow keeps task worktree even after task_run completion"
    );
    assert!(
        paths.contains(&PathBuf::from(
            "/tmp/wt-review-rr_0123456789ABCDEFGHJKMNPQRS-active"
        )),
        "unreleased reviewer worktree for running workflow is active"
    );
    assert!(
        paths.contains(&PathBuf::from("/tmp/wt-task-tr_11111111111111111111111111")),
        "failed workflow keeps task worktree for debugging"
    );
    assert!(
        paths.contains(&PathBuf::from(
            "/tmp/wt-review-rr_11111111111111111111111111-debug"
        )),
        "failed workflow keeps unreleased reviewer worktree for debugging"
    );
    assert!(
        !paths.contains(&PathBuf::from(
            "/tmp/wt-review-rr_0123456789ABCDEFGHJKMNPQRS-released"
        )),
        "released reviewer worktree is no longer active"
    );
    assert!(
        !paths.contains(&PathBuf::from("/tmp/wt-task-tr_22222222222222222222222222")),
        "finished workflow task worktree is boot-GC cleanup debris"
    );
    assert!(
        !paths.contains(&PathBuf::from(
            "/tmp/wt-review-rr_22222222222222222222222222-done"
        )),
        "finished workflow reviewer worktree is boot-GC cleanup debris"
    );
}

#[tokio::test]
async fn failed_worktree_cleanup_candidates_include_only_failed_runs() {
    let (s, _d) = store().await;

    let running = RunId::from_string("cleanup-running");
    s.insert_run(&running, "sha").await.expect("running run");
    let running_task = s
        .insert_task_run(&running, &NodeId::parsed("implement"))
        .await
        .expect("running task");
    let running_task_path = PathBuf::from(format!("/tmp/wt-task-{}", running_task.as_str()));
    s.set_task_run_worktree(&running_task, &running_task_path)
        .await
        .expect("running worktree");

    let failed = RunId::from_string("cleanup-failed");
    s.insert_run(&failed, "sha").await.expect("failed run");
    let failed_task = s
        .insert_task_run(&failed, &NodeId::parsed("implement"))
        .await
        .expect("failed task");
    let failed_task_path = PathBuf::from(format!("/tmp/wt-task-{}", failed_task.as_str()));
    s.set_task_run_worktree(&failed_task, &failed_task_path)
        .await
        .expect("failed task worktree");
    let failed_review = s
        .insert_review_run(&failed, &NodeId::parsed("review"), 2, 1, "csha")
        .await
        .expect("failed review");
    let failed_reviewer = AgentId::parsed("debug");
    let failed_review_path = PathBuf::from(format!(
        "/tmp/wt-review-{}-{}",
        failed_review.as_str(),
        failed_reviewer.as_str()
    ));
    s.set_review_worktree(&failed_review, &failed_reviewer, &failed_review_path)
        .await
        .expect("failed review worktree");
    let released_reviewer = AgentId::parsed("released");
    let released_review_path = PathBuf::from(format!(
        "/tmp/wt-review-{}-{}",
        failed_review.as_str(),
        released_reviewer.as_str()
    ));
    s.set_review_worktree(&failed_review, &released_reviewer, &released_review_path)
        .await
        .expect("released review worktree");
    assert!(
        s.take_review_worktree(&failed_review, &released_reviewer)
            .await
            .expect("take released reviewer")
            .is_some(),
        "released reviewer worktree existed before take"
    );
    s.update_run_status(&failed, RunStatus::Failed)
        .await
        .expect("mark failed");

    let finished = RunId::from_string("cleanup-finished");
    s.insert_run(&finished, "sha").await.expect("finished run");
    let finished_task = s
        .insert_task_run(&finished, &NodeId::parsed("implement"))
        .await
        .expect("finished task");
    let finished_task_path = PathBuf::from(format!("/tmp/wt-task-{}", finished_task.as_str()));
    s.set_task_run_worktree(&finished_task, &finished_task_path)
        .await
        .expect("finished worktree");
    s.update_run_status(&finished, RunStatus::Finished)
        .await
        .expect("mark finished");

    let candidates = s
        .failed_worktree_cleanup_candidates()
        .await
        .expect("cleanup candidates");
    let candidate_pairs: std::collections::HashSet<(String, PathBuf)> = candidates
        .iter()
        .map(|candidate| (candidate.key.clone(), candidate.path.clone()))
        .collect();

    assert_eq!(
        candidate_pairs,
        std::collections::HashSet::from([
            (failed_task.as_str().to_string(), failed_task_path),
            (
                format!(
                    "review-{}-{}",
                    failed_review.as_str(),
                    failed_reviewer.as_str()
                ),
                failed_review_path,
            ),
        ]),
        "only failed-run task and unreleased reviewer worktrees are cleanup candidates"
    );
}

#[tokio::test]
async fn task_run_complete_closes_park_parity() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r");
    let node = NodeId::parsed("impl");
    s.insert_run(&run, "sha").await.expect("run");
    let tr = s.insert_task_run(&run, &node).await.expect("task run");
    assert_eq!(
        s.lookup_park_by_task_run(&tr).await.expect("lookup"),
        Some((run.clone(), node.clone())),
        "open task run parks"
    );
    s.complete_task_run(&tr).await.expect("complete");
    assert!(
        s.lookup_park_by_task_run(&tr)
            .await
            .expect("lookup")
            .is_none(),
        "completed task run no longer parks"
    );
}

#[tokio::test]
async fn artifact_round_trips_content_addressed() {
    let (s, _d) = store().await;
    let art = Artifact {
        kind: ArtifactKind::Transcript,
        path: PathBuf::from("out.txt"),
        sha256: "abc123".to_string(),
        bytes: 5,
    };
    agentd_store::artifact_repo::insert_artifact(s.pool(), &art, None, None)
        .await
        .expect("insert");
    agentd_store::artifact_repo::insert_artifact(s.pool(), &art, None, None)
        .await
        .expect("idempotent on sha256");
    let back = agentd_store::artifact_repo::get_artifact(s.pool(), "abc123")
        .await
        .expect("get")
        .expect("some");
    assert_eq!(back.kind, ArtifactKind::Transcript);
    assert_eq!(back.bytes, 5);
    assert_eq!(back.path, PathBuf::from("out.txt"));
}

const CANONICAL: &str = r#"digraph m {
    "start" [shape=Mdiamond];
    "spec" [handler="wait.human", prompt="approve spec?"];
    "impl" [handler="codergen", role="implementer"];
    "review" [handler="parallel.fan_out", reviewers="claude-sec,codex-perf,gemini-readability"];
    "aggregate" [handler="parallel.fan_in", aggregator="majority_pass", goal_gate=true];
    "end" [shape=Msquare];
    "start" -> "spec";
    "spec" -> "impl" [condition="answer=approve"];
    "impl" -> "review";
    "review" -> "aggregate";
    "aggregate" -> "end";
}"#;

fn park_reason(progress: &RunProgress) -> &ParkReason {
    match progress {
        RunProgress::Parked { reason, .. } => reason,
        other => panic!("expected Parked, got {other:?}"),
    }
}

#[tokio::test]
async fn engine_runs_canonical_flow_against_sqlite_store() {
    let (store, _dir) = store().await;
    let backend = FakeBackend::new();
    let runner = RecordingCommandRunner::new();
    let mempal = MempalStub::new();
    let clock = FixedClock::new(0);
    let registry = HandlerRegistry::with_builtins();
    let ast = parser::parse(CANONICAL).expect("parse");
    let graph = NodeGraph::from_ast_unvalidated(&ast);
    let ports = Ports {
        backend: &backend,
        runner: &runner,
        store: &store,
        mempal: &mempal,
        clock: &clock,
    };
    let engine = Engine::new(&graph, &registry, ports, "sha-test");
    let run = RunId::from_string("run-1");

    // The full park/resume cycle, but persisting through the REAL SQLite store
    // (checkpoints, context_snapshot, and every park row round-trip via SQLite).
    let parked = engine.execute(&run).await.expect("execute");
    let wait_id = match park_reason(&parked) {
        ParkReason::HumanAnswer { wait_id } => wait_id.clone(),
        other => panic!("expected HumanAnswer, got {other:?}"),
    };
    let parked = engine
        .deliver_event(EngineEvent::HumanAnswered {
            wait_id,
            answer: "approve".to_string(),
            feedback: None,
        })
        .await
        .expect("approve");
    let task_run_id = match park_reason(&parked) {
        ParkReason::AgentOutcome { task_run_id } => task_run_id.clone(),
        other => panic!("expected AgentOutcome, got {other:?}"),
    };
    let parked = engine
        .deliver_event(EngineEvent::AgentOutcomeSubmitted {
            task_run_id,
            outcome: Outcome::success(),
        })
        .await
        .expect("agent outcome");
    let review_run_id = match park_reason(&parked) {
        ParkReason::ReviewVerdicts { review_run_id, .. } => review_run_id.clone(),
        other => panic!("expected ReviewVerdicts, got {other:?}"),
    };
    let mut last = None;
    for reviewer in ["claude-sec", "codex-perf", "gemini-readability"] {
        last = Some(
            engine
                .deliver_event(EngineEvent::ReviewVerdictSubmitted {
                    review_run_id: review_run_id.clone(),
                    reviewer_id: AgentId::parsed(reviewer),
                    verdict: VerdictValue::Pass,
                    findings: String::new(),
                })
                .await
                .expect("verdict"),
        );
    }
    assert_eq!(
        last.expect("final progress"),
        RunProgress::Finished {
            run_id: run.clone()
        },
        "the engine completes the canonical flow against the real SQLite store"
    );
    // The run row is persisted Finished.
    let status: String = sqlx::query_scalar("SELECT status FROM runs WHERE id = 'run-1'")
        .fetch_one(store.pool())
        .await
        .expect("status");
    assert_eq!(status, "finished");
}
