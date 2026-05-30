//! P0.4 Task 2: `insert_node_outcome` enqueues `Outcome.mempal_writes` into
//! `mempal_outbox` in the SAME transaction as the `node_outcomes` row (design
//! §3.4). Test names match `specs/mempal/p30-outbox-drainer.spec.md`.

use agentd_core::types::{MempalWrite, NodeId, Outcome, RunId};
use agentd_store::{SqliteStore, outbox_repo, outcome_repo, run_repo};

async fn store() -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let s = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    (s, dir)
}

fn outcome_with_writes(writes: Vec<MempalWrite>) -> Outcome {
    let mut outcome = Outcome::success();
    outcome.mempal_writes = writes;
    outcome
}

async fn count(pool: &sqlx::SqlitePool, table: &str, run: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table} WHERE run_id = ?"))
        .bind(run)
        .fetch_one(pool)
        .await
        .expect("count query")
}

#[tokio::test]
async fn test_kg_add_writes_outbox_row_in_same_tx_as_node_outcome() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r1");
    run_repo::insert_run(s.pool(), &run, "sha")
        .await
        .expect("run");
    let node = NodeId::parsed("draft");

    let outcome = outcome_with_writes(vec![MempalWrite::KgAdd {
        subject: "s".to_string(),
        predicate: "p".to_string(),
        object: "o".to_string(),
    }]);
    outcome_repo::insert_node_outcome(s.pool(), &run, &node, &outcome)
        .await
        .expect("insert");

    assert_eq!(count(s.pool(), "node_outcomes", "r1").await, 1);

    let pending = outbox_repo::claim_pending(s.pool(), 10)
        .await
        .expect("claim");
    assert_eq!(pending.len(), 1, "one outbox row per write");
    assert_eq!(pending[0].kind, "kg_add");
    assert_eq!(pending[0].attempts, 0);

    let write: MempalWrite =
        serde_json::from_str(&pending[0].payload).expect("payload round-trips to MempalWrite");
    match write {
        MempalWrite::KgAdd {
            subject,
            predicate,
            object,
        } => assert_eq!(
            (subject.as_str(), predicate.as_str(), object.as_str()),
            ("s", "p", "o")
        ),
        other => panic!("expected KgAdd, got {other:?}"),
    }
}

#[tokio::test]
async fn test_ingest_via_outbox_does_not_block_workflow() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r1");
    run_repo::insert_run(s.pool(), &run, "sha")
        .await
        .expect("run");
    let node = NodeId::parsed("draft");

    let outcome = outcome_with_writes(vec![MempalWrite::Ingest {
        wing: "proj".to_string(),
        kind: "spec".to_string(),
        body: "b".to_string(),
        importance: 5,
    }]);
    // insert_node_outcome contacts no network — it only enqueues a row.
    outcome_repo::insert_node_outcome(s.pool(), &run, &node, &outcome)
        .await
        .expect("insert");

    let pending = outbox_repo::claim_pending(s.pool(), 10)
        .await
        .expect("claim");
    assert_eq!(pending.len(), 1, "the write is pending, not sent");
    assert_eq!(pending[0].kind, "ingest");
}

#[tokio::test]
async fn enqueue_rolls_back_with_the_outcome_on_failure() {
    let (s, _d) = store().await;
    // No run inserted → the run_id foreign key violates, aborting the tx.
    let run = RunId::from_string("ghost");
    let node = NodeId::parsed("draft");

    let outcome = outcome_with_writes(vec![MempalWrite::KgAdd {
        subject: "s".to_string(),
        predicate: "p".to_string(),
        object: "o".to_string(),
    }]);
    let result = outcome_repo::insert_node_outcome(s.pool(), &run, &node, &outcome).await;
    assert!(result.is_err(), "a missing run FK should error");

    assert_eq!(count(s.pool(), "node_outcomes", "ghost").await, 0);
    assert_eq!(count(s.pool(), "mempal_outbox", "ghost").await, 0);
}

#[tokio::test]
async fn outcome_without_writes_enqueues_nothing() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r1");
    run_repo::insert_run(s.pool(), &run, "sha")
        .await
        .expect("run");
    let node = NodeId::parsed("draft");

    outcome_repo::insert_node_outcome(s.pool(), &run, &node, &Outcome::success())
        .await
        .expect("insert");

    assert_eq!(count(s.pool(), "node_outcomes", "r1").await, 1);
    let pending = outbox_repo::claim_pending(s.pool(), 10)
        .await
        .expect("claim");
    assert!(pending.is_empty());
}

#[tokio::test]
async fn enqueue_writes_one_row_per_write_in_order() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r1");
    run_repo::insert_run(s.pool(), &run, "sha")
        .await
        .expect("run");
    let node = NodeId::parsed("draft");

    let outcome = outcome_with_writes(vec![
        MempalWrite::KgAdd {
            subject: "s".to_string(),
            predicate: "p".to_string(),
            object: "o".to_string(),
        },
        MempalWrite::Ingest {
            wing: "w".to_string(),
            kind: "k".to_string(),
            body: "b".to_string(),
            importance: 1,
        },
    ]);
    outcome_repo::insert_node_outcome(s.pool(), &run, &node, &outcome)
        .await
        .expect("insert");

    let pending = outbox_repo::claim_pending(s.pool(), 10)
        .await
        .expect("claim");
    assert_eq!(pending.len(), 2, "one outbox row per write");
    assert_eq!(pending[0].kind, "kg_add", "FIFO: the first write first");
    assert_eq!(pending[1].kind, "ingest");
}
