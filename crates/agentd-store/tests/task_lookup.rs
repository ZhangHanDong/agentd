//! P0.9 9a-T1: `task_repo::find_open_task_run` — the forward (run,node)->task_run
//! read the production `RunHost`'s `open_task` needs. Names match
//! `specs/store/p6-find-open-task-run.spec.md`.

use agentd_core::ports::Store;
use agentd_core::types::{AgentId, NativeExecutionSpec, NodeId, RunId};
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
    assert_eq!(found.map(|(id, _worktree, _agent)| id), Some(tr));
}

#[tokio::test]
async fn set_task_run_worktree_persists_path() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r1");
    run_repo::insert_run(s.pool(), &run, "sha")
        .await
        .expect("run");
    let node = NodeId::from_string("implement");
    let tr = task_repo::insert_task_run(s.pool(), &run, &node)
        .await
        .expect("task run");
    let wt = "/tmp/wt-task";

    task_repo::set_task_run_worktree(s.pool(), &tr, wt)
        .await
        .expect("set worktree");

    let found = task_repo::find_open_task_run(s.pool(), &run, &node)
        .await
        .expect("find")
        .expect("open task run");
    assert_eq!(found.0, tr);
    assert_eq!(found.1.as_deref(), Some(wt));
}

#[tokio::test]
async fn insert_task_run_with_spec_is_atomic_and_readable_through_store_port() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r-spec");
    run_repo::insert_run(s.pool(), &run, "sha")
        .await
        .expect("run");
    let spec = NativeExecutionSpec {
        version: 1,
        provider: "codex".into(),
        program: "codex".into(),
        args: vec!["exec".into()],
        cwd: None,
        env: Vec::new(),
    };
    let task = Store::insert_task_run_with_spec(&s, &run, &NodeId::from_string("implement"), &spec)
        .await
        .expect("task with spec");
    assert_eq!(
        Store::get_task_execution_spec(&s, &task)
            .await
            .expect("read"),
        Some(spec)
    );
}

#[tokio::test]
async fn set_task_run_agent_persists_agent_id() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r1");
    run_repo::insert_run(s.pool(), &run, "sha")
        .await
        .expect("run");
    let node = NodeId::from_string("implement");
    let tr = task_repo::insert_task_run(s.pool(), &run, &node)
        .await
        .expect("task run");
    task_repo::set_task_run_worktree(s.pool(), &tr, "/tmp/wt-task")
        .await
        .expect("set worktree");

    task_repo::set_task_run_agent(s.pool(), &tr, &AgentId::parsed("implementer"))
        .await
        .expect("set task agent");

    let found = task_repo::find_open_task_run(s.pool(), &run, &node)
        .await
        .expect("find")
        .expect("open task run");
    assert_eq!(found.0, tr);
    assert_eq!(found.1.as_deref(), Some("/tmp/wt-task"));
    assert_eq!(found.2.as_deref(), Some("implementer"));
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

#[test]
fn p6_spec_marks_agent_id_gap_superseded() {
    let spec = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../specs/store/p6-find-open-task-run.spec.md"),
    )
    .expect("read p6 spec");
    assert!(
        spec.contains("P121") && spec.contains("agent_id"),
        "P6 should mark its old agent_id gap as superseded by P121:\n{spec}"
    );
}
