//! P0.9 9a: the production `RunHost` contract, exercised over a REAL `SqliteStore`
//! on a tempfile + the in-memory port fakes (NOT `FakeRunHost`). The full
//! `draft.dot` E2E + emit assertions land in 9a-T3; this skeleton checks
//! construction + a read.

use std::path::PathBuf;

use agentd_bin::{ProductionRunHost, SystemClock};
use agentd_core::engine::RunProgress;
use agentd_core::ports::RunStatus;
use agentd_core::test_support::{FakeBackend, MempalStub, RecordingCommandRunner};
use agentd_core::types::RunId;
use agentd_store::{SqliteStore, review_repo, run_repo};
use agentd_surface::host::RunHost;
use agentd_surface::mcp_server::dispatch;
use serde_json::json;

fn workflows_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workflows")
}

async fn production_host() -> (ProductionRunHost, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let host = ProductionRunHost::new(
        store,
        Box::new(FakeBackend::new()),
        Box::new(RecordingCommandRunner::new()),
        Box::new(MempalStub::new()),
        Box::new(SystemClock),
        workflows_dir(),
    );
    (host, dir)
}

#[tokio::test]
async fn production_run_snapshot_is_none_for_unknown_run() {
    let (host, _dir) = production_host().await;
    let snap = host
        .run_snapshot(&RunId::from_string("ghost"))
        .await
        .expect("run_snapshot");
    assert!(snap.is_none(), "an unknown run has no snapshot");
}

/// The scriptable in-process agent: submit a node's success through the same MCP
/// tool layer a real agent uses (`dispatch`), minus the rmcp wire.
async fn agent_submit_success(
    host: &ProductionRunHost,
    run: &str,
    node: &str,
) -> Result<serde_json::Value, agentd_surface::SurfaceError> {
    dispatch(
        host,
        "submit_outcome",
        json!({
            "run_id": run, "node_id": node, "attempt": 1, "status": "success",
            "context_updates": {}, "suggested_next": []
        }),
    )
    .await
}

/// Record a `draft.dot` run and start it (parks at `propose_spec`).
async fn start_draft(host: &ProductionRunHost, run: &RunId) -> RunProgress {
    run_repo::record_run(host.store().pool(), run, "draft.dot", "sha")
        .await
        .expect("record run");
    host.start_run(run).await.expect("start run")
}

#[tokio::test]
async fn production_runhost_drives_draft_dot_to_done() {
    let (host, _dir) = production_host().await;
    let run = RunId::from_string("r1");

    let parked = start_draft(&host, &run).await;
    assert!(
        matches!(parked, RunProgress::Parked { .. }),
        "draft.dot parks at propose_spec, got {parked:?}"
    );

    agent_submit_success(&host, "r1", "propose_spec")
        .await
        .expect("submit propose_spec");

    let snap = host
        .run_snapshot(&run)
        .await
        .expect("snapshot")
        .expect("run exists");
    assert_eq!(snap.status, "finished", "the run completed");

    let events = host.events_from(&run, 0).await.expect("events");
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["run_parked", "run_finished"],
        "one row per state change, in order"
    );
    assert!(events[0].seq < events[1].seq, "seq is increasing");
}

#[tokio::test]
async fn production_runhost_replayed_submit_is_rejected_without_new_event() {
    let (host, _dir) = production_host().await;
    let run = RunId::from_string("r1");
    start_draft(&host, &run).await;
    agent_submit_success(&host, "r1", "propose_spec")
        .await
        .expect("first submit");

    // Replay: the task is closed, so find_open_task_run -> None -> NotAssigned.
    let replay = agent_submit_success(&host, "r1", "propose_spec").await;
    assert!(
        replay.is_err(),
        "a replayed submit for a closed task is rejected, got {replay:?}"
    );

    let events = host.events_from(&run, 0).await.expect("events");
    assert_eq!(
        events.len(),
        2,
        "the rejected replay emits no additional event row"
    );
}

/// The scriptable reviewer: submit a pass verdict through the `submit_review`
/// tool (which also exercises the production host's `review_counts`).
async fn agent_submit_review(
    host: &ProductionRunHost,
    review_run_id: &str,
    reviewer: &str,
) -> Result<serde_json::Value, agentd_surface::SurfaceError> {
    dispatch(
        host,
        "submit_review",
        json!({
            "review_run_id": review_run_id, "reviewer_id": reviewer,
            "verdict": "pass", "findings": []
        }),
    )
    .await
}

#[tokio::test]
async fn production_runhost_drives_execute_dot_to_done() {
    let (host, _dir) = production_host().await;
    let run = RunId::from_string("e1");
    run_repo::record_run(host.store().pool(), &run, "execute.dot", "sha")
        .await
        .expect("record");

    // start -> pull_frozen_spec, draft_plan (tools) -> implement (codergen) parks.
    let parked = host.start_run(&run).await.expect("start");
    assert!(
        matches!(parked, RunProgress::Parked { .. }),
        "execute.dot parks at implement, got {parked:?}"
    );

    // implement success -> verify_lifecycle (tool) -> review (fan_out) parks.
    agent_submit_success(&host, "e1", "implement")
        .await
        .expect("submit implement");

    // The scriptable agent learns review_run_id from the store (the spawn-context
    // seam; the real rmcp path is D7/deployment).
    let review_run_id = review_repo::find_open_review_run(host.store().pool(), &run)
        .await
        .expect("find review run")
        .expect("an open review run");
    for reviewer in ["claude-sec", "codex-perf", "gemini-readability"] {
        agent_submit_review(&host, review_run_id.as_str(), reviewer)
            .await
            .expect("submit_review");
    }

    // aggregate (majority_pass) -> open_pr -> report_acceptance -> done.
    let snap = host
        .run_snapshot(&run)
        .await
        .expect("snapshot")
        .expect("run exists");
    assert_eq!(snap.status, "finished", "execute.dot reached done");

    // The multi-park emit sequence: parked at implement AND review, then finished.
    let events = host.events_from(&run, 0).await.expect("events");
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(
        kinds.first(),
        Some(&"run_parked"),
        "starts parked: {kinds:?}"
    );
    assert_eq!(
        kinds.last(),
        Some(&"run_finished"),
        "ends finished: {kinds:?}"
    );
    assert!(
        kinds.iter().filter(|k| **k == "run_parked").count() >= 2,
        "multi-park (implement + review): {kinds:?}"
    );
}

#[tokio::test]
async fn emit_persists_and_broadcasts() {
    let (host, _dir) = production_host().await;
    let run = RunId::from_string("r1");
    // Subscribe BEFORE the run starts so the live event is captured.
    let mut rx = host.subscribe_events();

    run_repo::record_run(host.store().pool(), &run, "draft.dot", "sha")
        .await
        .expect("record");
    host.start_run(&run).await.expect("start"); // parks at propose_spec -> emits run_parked

    // Persisted (durable/audit).
    let persisted = host.events_from(&run, 0).await.expect("events");
    assert_eq!(
        persisted.iter().filter(|e| e.kind == "run_parked").count(),
        1,
        "the event is persisted"
    );

    // Broadcast (the live tail) — the same event, non-blocking.
    let live = rx.try_recv().expect("a live event was broadcast");
    assert_eq!(live.run_id, "r1");
    assert_eq!(live.event.kind, "run_parked");
    assert_eq!(
        live.event.seq, persisted[0].seq,
        "same seq as the persisted row"
    );
}

#[tokio::test]
async fn production_list_runs_reflects_statuses() {
    let (host, _dir) = production_host().await;
    // run a: started -> parks (status stays "running"; only terminal updates it).
    let a = RunId::from_string("a");
    run_repo::record_run(host.store().pool(), &a, "draft.dot", "sha")
        .await
        .expect("record a");
    host.start_run(&a).await.expect("start a");
    // run b: recorded then marked finished.
    let b = RunId::from_string("b");
    run_repo::record_run(host.store().pool(), &b, "draft.dot", "sha")
        .await
        .expect("record b");
    run_repo::update_run_status(host.store().pool(), &b, RunStatus::Finished)
        .await
        .expect("finish b");

    let runs = host.list_runs().await.expect("list_runs");
    assert_eq!(runs.len(), 2, "both runs listed");
    assert!(
        runs[0].started_at >= runs[1].started_at,
        "most-recently-started first"
    );
    let a_sum = runs
        .iter()
        .find(|r| r.run_id == "a")
        .expect("run a present");
    let b_sum = runs
        .iter()
        .find(|r| r.run_id == "b")
        .expect("run b present");
    assert_eq!(a_sum.status, "running", "the started run is in-flight");
    assert_eq!(b_sum.status, "finished");
}

#[tokio::test]
async fn production_runhost_dedupes_same_node_reparks() {
    // P1 re-park-noise: a fan-out review re-parks at the SAME node per non-final
    // verdict; the emit point must dedup so only the first park at a node emits.
    let (host, _dir) = production_host().await;
    let run = RunId::from_string("e1");
    run_repo::record_run(host.store().pool(), &run, "execute.dot", "sha")
        .await
        .expect("record");

    // start -> implement park; implement success -> review (fan_out) park.
    host.start_run(&run).await.expect("start");
    agent_submit_success(&host, "e1", "implement")
        .await
        .expect("submit implement");
    let review_run_id = review_repo::find_open_review_run(host.store().pool(), &run)
        .await
        .expect("find review run")
        .expect("an open review run");

    // 1 of 3 pass verdicts (majority_pass) does NOT decide -> re-parks at review.
    agent_submit_review(&host, review_run_id.as_str(), "claude-sec")
        .await
        .expect("submit r1");
    // Confirm it WAS a same-node re-park (still at review), so the dedup assertion
    // below is meaningful and not vacuous.
    let mid = host
        .run_snapshot(&run)
        .await
        .expect("snap")
        .expect("exists");
    assert_eq!(
        mid.current_node.as_deref(),
        Some("review"),
        "the 1st verdict re-parks at the same review node"
    );

    // The remaining reviewers complete the review -> the run drives to done.
    for reviewer in ["codex-perf", "gemini-readability"] {
        agent_submit_review(&host, review_run_id.as_str(), reviewer)
            .await
            .expect("submit review");
    }
    let snap = host
        .run_snapshot(&run)
        .await
        .expect("snap")
        .expect("exists");
    assert_eq!(snap.status, "finished", "execute.dot reached done");

    // The same-node re-park emitted no duplicate: exactly one run_parked for review.
    let events = host.events_from(&run, 0).await.expect("events");
    let review_parks = events
        .iter()
        .filter(|e| e.kind == "run_parked" && e.payload == r#"{"node":"review"}"#)
        .count();
    assert_eq!(
        review_parks,
        1,
        "the same-node re-park is deduped to one run_parked: {:?}",
        events
            .iter()
            .map(|e| (e.kind.as_str(), e.payload.as_str()))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn production_runhost_events_from_unknown_run_is_empty() {
    let (host, _dir) = production_host().await;
    let events = host
        .events_from(&RunId::from_string("ghost"), 0)
        .await
        .expect("events");
    assert!(events.is_empty());
}
