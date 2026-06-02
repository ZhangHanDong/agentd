//! P0.9 9a-T1: `task_repo::find_open_task_run` — the forward (run,node)->task_run
//! read the production `RunHost`'s `open_task` needs. Names match
//! `specs/store/p6-find-open-task-run.spec.md`.

use agentd_core::types::{NodeId, RunId};
use agentd_store::{SqliteStore, run_repo, task_repo};

async fn store() -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let s = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    (s, dir)
}

#[tokio::test]
async fn find_open_task_run_returns_open_park() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r1");
    run_repo::insert_run(s.pool(), &run, "sha")
        .await
        .expect("run");
    let node = NodeId::from_string("implement");
    let tr = task_repo::insert_task_run(s.pool(), &run, &node)
        .await
        .expect("task run");

    let found = task_repo::find_open_task_run(s.pool(), &run, &node)
        .await
        .expect("find");
    assert_eq!(found.map(|(id, _worktree)| id), Some(tr));
}

#[tokio::test]
async fn find_open_task_run_is_none_after_complete() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r1");
    run_repo::insert_run(s.pool(), &run, "sha")
        .await
        .expect("run");
    let node = NodeId::from_string("implement");
    let tr = task_repo::insert_task_run(s.pool(), &run, &node)
        .await
        .expect("task run");
    task_repo::complete_task_run(s.pool(), &tr)
        .await
        .expect("complete");

    let found = task_repo::find_open_task_run(s.pool(), &run, &node)
        .await
        .expect("find");
    assert!(found.is_none(), "a completed task run is not open");
}

#[tokio::test]
async fn find_open_task_run_is_none_for_unknown_node() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r1");
    run_repo::insert_run(s.pool(), &run, "sha")
        .await
        .expect("run");
    task_repo::insert_task_run(s.pool(), &run, &NodeId::from_string("implement"))
        .await
        .expect("task run");

    let found = task_repo::find_open_task_run(s.pool(), &run, &NodeId::from_string("review"))
        .await
        .expect("find");
    assert!(found.is_none(), "no open task run for a different node");
}
