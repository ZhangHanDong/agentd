//! Task 2: runs + `node_outcomes` + checkpoint repos. Names match the spec.

use std::collections::BTreeMap;

use agentd_core::engine::Checkpoint;
use agentd_core::ports::RunStatus;
use agentd_core::types::{Artifact, ArtifactKind, NodeId, Outcome, RunContext, RunId, Status};
use agentd_store::{SqliteStore, StoreError, checkpoint_repo, outcome_repo, run_repo};
use sqlx::Row;

async fn store() -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let s = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    (s, dir)
}

#[tokio::test]
async fn insert_run_minimal_row_satisfies_reconciled_schema() {
    // The reconciliation proof: insert_run supplies only id + workflow_sha; every
    // other NOT NULL is store-filled or the column is nullable. A failed insert
    // here would point at the exact unreconciled column.
    let (s, _d) = store().await;
    let run = RunId::from_string("r1");
    run_repo::insert_run(s.pool(), &run, "sha-abc")
        .await
        .expect("minimal insert_run must satisfy the schema");
    let row = sqlx::query(
        "SELECT workflow_sha, status, project_id, workflow_path FROM runs WHERE id = ?",
    )
    .bind("r1")
    .fetch_one(s.pool())
    .await
    .expect("row");
    assert_eq!(row.get::<String, _>("workflow_sha"), "sha-abc");
    assert_eq!(row.get::<String, _>("status"), "running");
    assert!(row.get::<Option<String>, _>("project_id").is_none());
    assert!(row.get::<Option<String>, _>("workflow_path").is_none());
}

#[tokio::test]
async fn insert_run_is_idempotent_on_id() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r1");
    run_repo::insert_run(s.pool(), &run, "sha1")
        .await
        .expect("first");
    run_repo::insert_run(s.pool(), &run, "sha2")
        .await
        .expect("second insert is a no-op, not a PK error");
    let sha: String = sqlx::query_scalar("SELECT workflow_sha FROM runs WHERE id = 'r1'")
        .fetch_one(s.pool())
        .await
        .expect("sha");
    assert_eq!(sha, "sha1", "first write wins");
}

#[tokio::test]
async fn update_run_status_errors_on_unknown_run() {
    let (s, _d) = store().await;
    let err =
        run_repo::update_run_status(s.pool(), &RunId::from_string("ghost"), RunStatus::Finished)
            .await
            .expect_err("unknown run must error");
    assert!(matches!(err, StoreError::NotFound), "got {err:?}");
}

#[tokio::test]
async fn run_status_and_current_node_round_trip() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r1");
    run_repo::insert_run(s.pool(), &run, "sha")
        .await
        .expect("run");
    run_repo::set_current_node(s.pool(), &run, &NodeId::parsed("review"))
        .await
        .expect("set node");
    run_repo::update_run_status(s.pool(), &run, RunStatus::Finished)
        .await
        .expect("status");
    let row = sqlx::query("SELECT status, current_node, finished_at FROM runs WHERE id = 'r1'")
        .fetch_one(s.pool())
        .await
        .expect("row");
    assert_eq!(row.get::<String, _>("status"), "finished");
    assert_eq!(row.get::<String, _>("current_node"), "review");
    assert!(row.get::<Option<i64>, _>("finished_at").is_some());
}

#[tokio::test]
async fn node_outcome_attempt_increments_and_latest_wins() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r1");
    let node = NodeId::parsed("impl");
    run_repo::insert_run(s.pool(), &run, "sha")
        .await
        .expect("run");
    outcome_repo::insert_node_outcome(s.pool(), &run, &node, &Outcome::fail())
        .await
        .expect("attempt 1");
    outcome_repo::insert_node_outcome(s.pool(), &run, &node, &Outcome::success())
        .await
        .expect("attempt 2");
    assert_eq!(
        outcome_repo::count_attempts(s.pool(), &run, &node)
            .await
            .expect("count"),
        2
    );
    let latest = outcome_repo::latest_outcome(s.pool(), &run, &node)
        .await
        .expect("latest")
        .expect("some");
    assert_eq!(latest.status, Status::Success, "highest attempt wins");
}

#[tokio::test]
async fn node_outcome_round_trips_context_label_and_artifact() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r1");
    let node = NodeId::parsed("build");
    run_repo::insert_run(s.pool(), &run, "sha")
        .await
        .expect("run");
    let mut outcome = Outcome::success();
    outcome.preferred_label = Some("go".to_string());
    outcome
        .context_updates
        .insert("k".to_string(), serde_json::Value::String("v".to_string()));
    outcome.artifacts.push(Artifact {
        kind: ArtifactKind::Transcript,
        path: std::path::PathBuf::from("out.txt"),
        sha256: "deadbeef".to_string(),
        bytes: 3,
    });
    outcome_repo::insert_node_outcome(s.pool(), &run, &node, &outcome)
        .await
        .expect("insert");
    let back = outcome_repo::latest_outcome(s.pool(), &run, &node)
        .await
        .expect("latest")
        .expect("some");
    assert_eq!(back.preferred_label.as_deref(), Some("go"));
    assert_eq!(
        back.context_updates.get("k").and_then(|v| v.as_str()),
        Some("v")
    );
    assert_eq!(back.artifacts.len(), 1);
    assert_eq!(back.artifacts[0].sha256, "deadbeef");
}

#[tokio::test]
async fn checkpoint_round_trips_through_store() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r1");
    run_repo::insert_run(s.pool(), &run, "sha")
        .await
        .expect("run");
    let mut retry = BTreeMap::new();
    retry.insert(NodeId::parsed("impl"), 2_u32);
    let mut ctx = RunContext::new();
    ctx.set("answer", serde_json::Value::String("approve".to_string()));
    let cp = Checkpoint {
        run_id: run.clone(),
        current_node: NodeId::parsed("review"),
        completed_nodes: vec![NodeId::parsed("start"), NodeId::parsed("impl")],
        retry_counts: retry,
        context_snapshot: ctx,
        workflow_sha: "sha".to_string(),
    };
    checkpoint_repo::write_checkpoint(s.pool(), &cp)
        .await
        .expect("write");
    let back = checkpoint_repo::load_checkpoint(s.pool(), &run)
        .await
        .expect("load")
        .expect("some");
    assert_eq!(back, cp);
    // Upsert: a second write replaces, not errors.
    let mut cp2 = cp.clone();
    cp2.current_node = NodeId::parsed("aggregate");
    checkpoint_repo::write_checkpoint(s.pool(), &cp2)
        .await
        .expect("upsert");
    let back2 = checkpoint_repo::load_checkpoint(s.pool(), &run)
        .await
        .expect("load2")
        .expect("some");
    assert_eq!(back2.current_node, NodeId::parsed("aggregate"));
}
