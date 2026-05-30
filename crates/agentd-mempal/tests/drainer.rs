//! P0.4 Task 3: the background outbox drainer (design §3.4). `drain_once` claims
//! pending rows FIFO, dispatches each to the `MempalClient`, marks drained on
//! success, and retries (with an attempt bound + alert) on failure. A down
//! mempal never stalls the workflow. Test names match
//! `specs/mempal/p30-outbox-drainer.spec.md`.

use agentd_core::CoreError;
use agentd_core::ports::{DrawerHit, MempalClient};
use agentd_core::test_support::MempalStub;
use agentd_core::types::{MempalWrite, NodeId, Outcome, RunId};
use agentd_store::{SqliteStore, outbox_repo, outcome_repo, run_repo};

use agentd_mempal::drainer::{DrainerConfig, drain_once};

/// A `MempalClient` that always reports mempal unreachable.
struct AlwaysErrClient;

#[async_trait::async_trait]
impl MempalClient for AlwaysErrClient {
    async fn search(&self, _: &str, _: &str, _: &str) -> Result<Vec<DrawerHit>, CoreError> {
        Err(CoreError::Mempal("down".to_string()))
    }
    async fn ingest(&self, _: &str, _: &str, _: &str) -> Result<(), CoreError> {
        Err(CoreError::Mempal("down".to_string()))
    }
    async fn kg_add(&self, _: &str, _: &str, _: &str) -> Result<(), CoreError> {
        Err(CoreError::Mempal("down".to_string()))
    }
    async fn fact_check(&self, _: &str) -> Result<Vec<DrawerHit>, CoreError> {
        Err(CoreError::Mempal("down".to_string()))
    }
}

async fn store_with_writes(
    writes: Vec<(NodeId, Vec<MempalWrite>)>,
) -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let s = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let run = RunId::from_string("r1");
    run_repo::insert_run(s.pool(), &run, "sha")
        .await
        .expect("run");
    for (node, ws) in writes {
        let mut outcome = Outcome::success();
        outcome.mempal_writes = ws;
        outcome_repo::insert_node_outcome(s.pool(), &run, &node, &outcome)
            .await
            .expect("insert");
    }
    (s, dir)
}

#[tokio::test]
async fn drainer_drains_pending_rows_and_marks_drained() {
    let (s, _d) = store_with_writes(vec![
        (
            NodeId::parsed("a"),
            vec![MempalWrite::KgAdd {
                subject: "s".to_string(),
                predicate: "p".to_string(),
                object: "o".to_string(),
            }],
        ),
        (
            NodeId::parsed("b"),
            vec![MempalWrite::Ingest {
                wing: "w".to_string(),
                kind: "k".to_string(),
                body: "b".to_string(),
                importance: 3,
            }],
        ),
    ])
    .await;

    let stub = MempalStub::new();
    let report = drain_once(&s, &stub, &DrainerConfig::default())
        .await
        .expect("drain ok");

    assert_eq!(report.drained, 2);
    assert!(report.alerts.is_empty());
    assert!(
        outbox_repo::claim_pending(s.pool(), 10)
            .await
            .expect("claim")
            .is_empty(),
        "every row drained"
    );
    assert_eq!(stub.kg_triples().len(), 1, "kg_add dispatched");
    assert_eq!(stub.ingested().len(), 1, "ingest dispatched");
}

#[tokio::test]
async fn test_drainer_retries_with_backoff_until_attempts_exceeded() {
    let (s, _d) = store_with_writes(vec![(
        NodeId::parsed("a"),
        vec![MempalWrite::Ingest {
            wing: "w".to_string(),
            kind: "k".to_string(),
            body: "b".to_string(),
            importance: 1,
        }],
    )])
    .await;

    let client = AlwaysErrClient;
    let cfg = DrainerConfig::default();
    let mut alerted = false;
    for _ in 0..8 {
        let report = drain_once(&s, &client, &cfg)
            .await
            .expect("drain returns Ok even when mempal errors");
        assert_eq!(report.drained, 0);
        if !report.alerts.is_empty() {
            alerted = true;
        }
    }
    assert!(alerted, "the exhausted row was reported as an alert");

    let pending = outbox_repo::claim_pending(s.pool(), 10)
        .await
        .expect("claim");
    assert_eq!(pending.len(), 1, "the row stays in the table");
    assert_eq!(
        pending[0].attempts, 6,
        "attempts capped past the bound of 5"
    );
}

#[tokio::test]
async fn drainer_tolerates_mempal_down() {
    let (s, _d) = store_with_writes(vec![(
        NodeId::parsed("a"),
        vec![MempalWrite::KgAdd {
            subject: "s".to_string(),
            predicate: "p".to_string(),
            object: "o".to_string(),
        }],
    )])
    .await;

    let client = AlwaysErrClient;
    let report = drain_once(&s, &client, &DrainerConfig::default())
        .await
        .expect("drain returns Ok, not Err, when mempal is down");
    assert_eq!(report.drained, 0);
    assert_eq!(report.retried, 1);

    let pending = outbox_repo::claim_pending(s.pool(), 10)
        .await
        .expect("claim");
    assert_eq!(pending.len(), 1, "the row is still pending");
    assert_eq!(pending[0].attempts, 1, "attempts bumped, nothing drained");
}

#[tokio::test]
async fn drainer_does_not_reclaim_exhausted_rows() {
    let (s, _d) = store_with_writes(vec![(
        NodeId::parsed("a"),
        vec![MempalWrite::Ingest {
            wing: "w".to_string(),
            kind: "k".to_string(),
            body: "b".to_string(),
            importance: 1,
        }],
    )])
    .await;

    let client = AlwaysErrClient;
    let cfg = DrainerConfig::default();
    for _ in 0..7 {
        drain_once(&s, &client, &cfg).await.expect("drain");
    }

    // Past the bound, the row is no longer retryable (so it cannot starve the
    // window) but it stays in the table for the operator to inspect.
    let retryable = outbox_repo::claim_retryable(s.pool(), 100, cfg.max_attempts)
        .await
        .expect("retryable");
    assert!(retryable.is_empty(), "an exhausted row is not re-claimed");

    let all = outbox_repo::claim_pending(s.pool(), 100)
        .await
        .expect("pending");
    assert_eq!(all.len(), 1, "the stuck row stays in the table");
    assert!(all[0].attempts > cfg.max_attempts);
}

#[tokio::test]
async fn drainer_drains_a_fact_check_write() {
    let (s, _d) = store_with_writes(vec![(
        NodeId::parsed("a"),
        vec![MempalWrite::FactCheck {
            text: "check me".to_string(),
        }],
    )])
    .await;

    let stub = MempalStub::new();
    let report = drain_once(&s, &stub, &DrainerConfig::default())
        .await
        .expect("drain ok");
    assert_eq!(
        report.drained, 1,
        "the fact_check write dispatched and drained"
    );
    assert!(
        outbox_repo::claim_pending(s.pool(), 10)
            .await
            .expect("claim")
            .is_empty()
    );
}
