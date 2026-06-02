//! P0.9 9a: the production `RunHost` contract, exercised over a REAL `SqliteStore`
//! on a tempfile + the in-memory port fakes (NOT `FakeRunHost`). The full
//! `draft.dot` E2E + emit assertions land in 9a-T3; this skeleton checks
//! construction + a read.

use std::path::PathBuf;

use agentd_bin::{ProductionRunHost, SystemClock};
use agentd_core::engine::RunProgress;
use agentd_core::test_support::{FakeBackend, MempalStub, RecordingCommandRunner};
use agentd_core::types::RunId;
use agentd_store::{SqliteStore, run_repo};
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

#[tokio::test]
async fn production_runhost_events_from_unknown_run_is_empty() {
    let (host, _dir) = production_host().await;
    let events = host
        .events_from(&RunId::from_string("ghost"), 0)
        .await
        .expect("events");
    assert!(events.is_empty());
}
