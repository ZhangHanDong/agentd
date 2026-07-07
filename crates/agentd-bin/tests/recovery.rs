//! P0.9 9d: the disaster-recovery drill — kill-9 resume (simulated by dropping +
//! reopening the same `SqliteStore`) + the signature idempotent-replay + the sha
//! guard. Names match `specs/e2e/p92-kill9-replay.spec.md`.

use std::path::{Path, PathBuf};

use agentd_bin::{ProductionRunHost, SystemClock};
use agentd_core::engine::{EngineEvent, RunProgress};
use agentd_core::ports::Store;
use agentd_core::test_support::{FakeBackend, MempalStub, RecordingCommandRunner};
use agentd_core::types::{NodeId, Outcome, RunId};
use agentd_store::{SqliteStore, outcome_repo, run_repo, task_repo};
use agentd_surface::host::RunHost;

fn workflows_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workflows")
}

/// A production host over the `SqliteStore` at `db` (reopening the same file
/// after a drop is the simulated restart).
async fn host_at(db: &Path) -> ProductionRunHost {
    let store = SqliteStore::connect(db).await.expect("connect");
    ProductionRunHost::new(
        store,
        Box::new(FakeBackend::new()),
        Box::new(RecordingCommandRunner::new()),
        Box::new(MempalStub::new()),
        Box::new(SystemClock),
        workflows_dir(),
    )
}

/// The open `propose_spec` task run's id for `run`.
async fn open_propose_spec(host: &ProductionRunHost, run: &RunId) -> agentd_core::types::TaskRunId {
    task_repo::find_open_task_run(
        host.store().pool(),
        run,
        &NodeId::from_string("propose_spec"),
    )
    .await
    .expect("find")
    .expect("an open propose_spec task")
    .0
}

#[tokio::test]
async fn kill9_resume_continues_from_checkpoint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("agentd.db");
    let run = RunId::from_string("r1");

    // Boot 1: start draft.dot and park at propose_spec, then drop the host
    // (closes the pool — the simulated SIGKILL).
    {
        let host = host_at(&db).await;
        run_repo::record_run(host.store().pool(), &run, "draft.dot", "sha")
            .await
            .expect("record");
        let parked = host.start_run(&run).await.expect("start");
        assert!(
            matches!(parked, RunProgress::Parked { .. }),
            "parked: {parked:?}"
        );
    }

    // Boot 2: reopen the SAME database with a fresh host and resolve the park.
    let host = host_at(&db).await;
    let task_run_id = open_propose_spec(&host, &run).await;
    let done = host
        .deliver(EngineEvent::AgentOutcomeSubmitted {
            task_run_id,
            outcome: Outcome::success(),
        })
        .await
        .expect("deliver");
    assert!(
        matches!(done, RunProgress::Finished { .. }),
        "resumed to done across the restart, got {done:?}"
    );

    // A node completed before the restart did not re-run.
    let attempts = outcome_repo::count_attempts(
        host.store().pool(),
        &run,
        &NodeId::from_string("fetch_issue_context"),
    )
    .await
    .expect("count");
    assert_eq!(attempts, 1, "the pre-restart tool node did not re-run");

    // The event log is continuous across the boundary.
    let events = host.events_from(&run, 0).await.expect("events");
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, vec!["run_parked", "run_finished"]);
    assert!(
        events[0].seq < events[1].seq,
        "event seq continuous across restart"
    );
}

#[tokio::test]
async fn replay_after_resume_is_ignored_without_duplicate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("agentd.db");
    let run = RunId::from_string("r1");
    let host = host_at(&db).await;
    run_repo::record_run(host.store().pool(), &run, "draft.dot", "sha")
        .await
        .expect("record");
    host.start_run(&run).await.expect("start");
    let task_run_id = open_propose_spec(&host, &run).await;

    let event = || EngineEvent::AgentOutcomeSubmitted {
        task_run_id: task_run_id.clone(),
        outcome: Outcome::success(),
    };
    let first = host.deliver(event()).await.expect("first deliver");
    let second = host.deliver(event()).await.expect("replay deliver");
    assert!(
        matches!(first, RunProgress::Finished { .. }),
        "first: {first:?}"
    );
    assert!(
        matches!(second, RunProgress::Ignored { .. }),
        "the replayed event is Ignored, got {second:?}"
    );

    let attempts = outcome_repo::count_attempts(
        host.store().pool(),
        &run,
        &NodeId::from_string("propose_spec"),
    )
    .await
    .expect("count");
    assert_eq!(attempts, 1, "the replay created no duplicate outcome");
    let events = host.events_from(&run, 0).await.expect("events");
    assert_eq!(events.len(), 2, "the replayed Ignored emitted no event");
}

#[tokio::test]
async fn resume_guard_gates_a_changed_workflow_sha() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("agentd.db");
    let run = RunId::from_string("r1");
    let host = host_at(&db).await;
    run_repo::record_run(host.store().pool(), &run, "draft.dot", "sha")
        .await
        .expect("record");
    host.start_run(&run).await.expect("start");

    let checkpoint = host
        .store()
        .load_checkpoint(&run)
        .await
        .expect("load")
        .expect("a checkpoint after parking");

    assert!(
        checkpoint
            .resume_guard(&checkpoint.workflow_sha, false)
            .is_ok(),
        "the matching sha resumes"
    );
    assert!(
        checkpoint.resume_guard("changed-sha", false).is_err(),
        "a changed sha is rejected without accept_change"
    );
    assert!(
        checkpoint.resume_guard("changed-sha", true).is_ok(),
        "accept_change overrides the sha guard"
    );
}
