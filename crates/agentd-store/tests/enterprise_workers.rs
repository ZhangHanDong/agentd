use agentd_core::types::{WorkerId, WorkerIncarnationId, WorkerStatus};
use agentd_store::worker_repo::{self, WorkerCreate, WorkerHeartbeatOutcome, WorkerRegistration};
use agentd_store::{SqliteStore, StoreError};
use serde_json::json;

async fn store() -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    (store, dir)
}

fn worker(id: WorkerId) -> WorkerCreate {
    WorkerCreate {
        id,
        trust_domain: "corp-coding".to_string(),
        labels: json!({"team": "runtime"}),
    }
}

fn registration(id: WorkerIncarnationId, host: &str) -> WorkerRegistration {
    WorkerRegistration {
        id,
        daemon_version: "0.0.0-p267".to_string(),
        host_name: host.to_string(),
        network_zone: Some("dev".to_string()),
        capabilities: json!({"runtime": ["codex"]}),
    }
}

#[tokio::test]
async fn worker_registration_supersedes_incarnation_and_rejects_stale_heartbeat() {
    let (store, _dir) = store().await;
    let worker_id = WorkerId::new();
    worker_repo::create_worker(store.pool(), worker(worker_id.clone()))
        .await
        .expect("enroll worker");

    let first_id = WorkerIncarnationId::new();
    let first = worker_repo::register_incarnation(
        store.pool(),
        &worker_id,
        registration(first_id.clone(), "host-a"),
    )
    .await
    .expect("first registration");
    assert!(first.is_current);
    assert!(first.superseded_at.is_none());
    let first_seen = first.last_seen_at;

    let second_id = WorkerIncarnationId::new();
    let second = worker_repo::register_incarnation(
        store.pool(),
        &worker_id,
        registration(second_id.clone(), "host-b"),
    )
    .await
    .expect("second registration");
    assert!(second.is_current);
    assert_eq!(second.worker_id, worker_id);
    assert_eq!(second.host_name, "host-b");

    let superseded = worker_repo::get_incarnation(store.pool(), &first_id)
        .await
        .expect("get first")
        .expect("first exists");
    assert!(!superseded.is_current);
    assert!(superseded.superseded_at.is_some());
    assert_eq!(superseded.last_seen_at, first_seen);
    assert_eq!(
        worker_repo::current_incarnation(store.pool(), &worker_id)
            .await
            .expect("current")
            .expect("current exists")
            .id,
        second_id
    );

    let stale = worker_repo::heartbeat_incarnation(store.pool(), &worker_id, &first_id)
        .await
        .expect("stale heartbeat outcome");
    assert_eq!(stale, WorkerHeartbeatOutcome::Stale);
    let unchanged = worker_repo::get_incarnation(store.pool(), &first_id)
        .await
        .expect("get first")
        .expect("first exists");
    assert_eq!(unchanged.last_seen_at, first_seen);

    let accepted = worker_repo::heartbeat_incarnation(store.pool(), &worker_id, &second_id)
        .await
        .expect("current heartbeat");
    assert!(matches!(accepted, WorkerHeartbeatOutcome::Accepted(_)));
    let worker = worker_repo::get_worker(store.pool(), &worker_id)
        .await
        .expect("get worker")
        .expect("worker exists");
    assert_eq!(worker.status, WorkerStatus::Online);
    assert_eq!(worker.record_version, 3);
}

#[tokio::test]
async fn retired_worker_rejects_new_incarnation() {
    let (store, _dir) = store().await;
    let worker_id = WorkerId::new();
    worker_repo::create_worker(store.pool(), worker(worker_id.clone()))
        .await
        .expect("enroll worker");
    worker_repo::register_incarnation(
        store.pool(),
        &worker_id,
        registration(WorkerIncarnationId::new(), "host-a"),
    )
    .await
    .expect("register");

    let retired =
        worker_repo::transition_worker_status(store.pool(), &worker_id, WorkerStatus::Retired)
            .await
            .expect("retire");
    assert_eq!(retired.status, WorkerStatus::Retired);
    assert_eq!(retired.record_version, 3);
    assert!(
        worker_repo::current_incarnation(store.pool(), &worker_id)
            .await
            .expect("current")
            .is_none()
    );

    let error = worker_repo::register_incarnation(
        store.pool(),
        &worker_id,
        registration(WorkerIncarnationId::new(), "host-b"),
    )
    .await
    .expect_err("retired worker must reject registration");
    assert!(matches!(error, StoreError::Conflict(_)), "got {error:?}");

    let unchanged = worker_repo::get_worker(store.pool(), &worker_id)
        .await
        .expect("get")
        .expect("worker");
    assert_eq!(unchanged.status, WorkerStatus::Retired);
    assert_eq!(unchanged.record_version, 3);
}
