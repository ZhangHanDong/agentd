use agentd_core::ports::{
    WorkerFleetDrainRequest, WorkerFleetHeartbeat, WorkerFleetHeartbeatResult, WorkerFleetPort,
    WorkerFleetPullRequest, WorkerFleetRegisterRequest,
};
use agentd_core::types::{NodeId, RunId, WorkerId, WorkerIncarnationId, WorkerStatus};
use agentd_store::SqliteStore;
use agentd_store::worker_fleet::SqliteWorkerFleet;
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use agentd_store::{run_repo, task_repo};
use serde_json::json;

#[tokio::test]
async fn worker_fleet_registers_and_rejects_stale_incarnation_heartbeats() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let fleet = SqliteWorkerFleet::new(store.pool().clone());
    let worker_id = WorkerId::new();
    let first_incarnation = WorkerIncarnationId::new();
    fleet
        .register(&WorkerFleetRegisterRequest {
            auth_proof: String::new(),
            worker_id: worker_id.clone(),
            trust_domain: "corp".into(),
            labels: json!({"region": "cn-east"}),
            incarnation_id: first_incarnation.clone(),
            daemon_version: "test".into(),
            host_name: "host-a".into(),
            network_zone: Some("dev".into()),
            capabilities: json!({"runtime": ["native"]}),
            capacity: 1,
            protocol_version: agentd_core::ports::WORKER_PROTOCOL_VERSION,
        })
        .await
        .expect("register");
    assert!(matches!(
        fleet
            .heartbeat(&WorkerFleetHeartbeat {
                auth_proof: String::new(),
                worker_id: worker_id.clone(),
                incarnation_id: first_incarnation.clone(),
            })
            .await
            .expect("heartbeat"),
        WorkerFleetHeartbeatResult::Accepted { .. }
    ));

    let second_incarnation = WorkerIncarnationId::new();
    fleet
        .register(&WorkerFleetRegisterRequest {
            auth_proof: String::new(),
            worker_id: worker_id.clone(),
            trust_domain: "corp".into(),
            labels: json!({}),
            incarnation_id: second_incarnation,
            daemon_version: "test".into(),
            host_name: "host-b".into(),
            network_zone: None,
            capabilities: json!({"runtime": ["native"]}),
            capacity: 1,
            protocol_version: agentd_core::ports::WORKER_PROTOCOL_VERSION,
        })
        .await
        .expect("re-register");
    assert_eq!(
        fleet
            .heartbeat(&WorkerFleetHeartbeat {
                auth_proof: String::new(),
                worker_id,
                incarnation_id: first_incarnation,
            })
            .await
            .expect("stale heartbeat"),
        WorkerFleetHeartbeatResult::Stale
    );
}

#[tokio::test]
async fn worker_fleet_can_drain_and_resume_current_incarnation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let fleet = SqliteWorkerFleet::new(store.pool().clone());
    let worker_id = WorkerId::new();
    let incarnation_id = WorkerIncarnationId::new();
    fleet
        .register(&WorkerFleetRegisterRequest {
            auth_proof: String::new(),
            worker_id: worker_id.clone(),
            trust_domain: "local".into(),
            labels: json!({}),
            incarnation_id: incarnation_id.clone(),
            daemon_version: "test".into(),
            host_name: "host".into(),
            network_zone: None,
            capabilities: json!({}),
            capacity: 1,
            protocol_version: agentd_core::ports::WORKER_PROTOCOL_VERSION,
        })
        .await
        .expect("register");

    fleet
        .set_drain(&WorkerFleetDrainRequest {
            auth_proof: String::new(),
            worker_id: worker_id.clone(),
            incarnation_id: incarnation_id.clone(),
            drain: true,
        })
        .await
        .expect("drain");
    assert_eq!(
        worker_repo::get_worker(store.pool(), &worker_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorkerStatus::Draining
    );
    fleet
        .set_drain(&WorkerFleetDrainRequest {
            auth_proof: String::new(),
            worker_id,
            incarnation_id,
            drain: false,
        })
        .await
        .expect("resume");
}

#[tokio::test]
async fn worker_fleet_recovers_workers_missing_heartbeats_to_offline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let fleet = SqliteWorkerFleet::new(store.pool().clone());
    let worker_id = WorkerId::new();
    let incarnation_id = WorkerIncarnationId::new();
    fleet
        .register(&WorkerFleetRegisterRequest {
            auth_proof: String::new(),
            worker_id: worker_id.clone(),
            trust_domain: "local".into(),
            labels: json!({}),
            incarnation_id,
            daemon_version: "test".into(),
            host_name: "host".into(),
            network_zone: None,
            capabilities: json!({}),
            capacity: 1,
            protocol_version: agentd_core::ports::WORKER_PROTOCOL_VERSION,
        })
        .await
        .expect("register");
    sqlx::query("UPDATE worker_incarnations SET last_seen_at = 1 WHERE worker_id = ?")
        .bind(worker_id.as_str())
        .execute(store.pool())
        .await
        .expect("age heartbeat");
    assert_eq!(fleet.recover_offline(2).await.expect("recover"), 1);
    assert_eq!(
        worker_repo::get_worker(store.pool(), &worker_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorkerStatus::Offline
    );
}

#[tokio::test]
async fn worker_fleet_pull_selects_oldest_unleased_open_task() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let fleet = SqliteWorkerFleet::new(store.pool().clone());
    let run_id = RunId::new();
    run_repo::insert_run(store.pool(), &run_id, "workflow-sha")
        .await
        .expect("run");
    let task_id = task_repo::insert_task_run(store.pool(), &run_id, &NodeId::parsed("impl"))
        .await
        .expect("task");
    let worker_id = WorkerId::new();
    let incarnation_id = WorkerIncarnationId::new();
    fleet
        .register(&WorkerFleetRegisterRequest {
            auth_proof: String::new(),
            worker_id,
            trust_domain: "local".into(),
            labels: json!({}),
            incarnation_id: incarnation_id.clone(),
            daemon_version: "test".into(),
            host_name: "host".into(),
            network_zone: None,
            capabilities: json!({}),
            capacity: 1,
            protocol_version: agentd_core::ports::WORKER_PROTOCOL_VERSION,
        })
        .await
        .expect("register");
    let grant = fleet
        .pull(&WorkerFleetPullRequest {
            auth_proof: String::new(),
            worker_incarnation_id: incarnation_id,
            observed_at: 10,
            expires_at: 20,
            request_id: None,
        })
        .await
        .expect("pull")
        .expect("queued task");
    assert_eq!(grant.execution_task_id, task_id);
}

#[tokio::test]
async fn worker_fleet_rejects_invalid_auth_proof() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let fleet = SqliteWorkerFleet::new(store.pool().clone()).with_auth_proof("fleet-secret");
    let error = fleet
        .register(&WorkerFleetRegisterRequest {
            auth_proof: "wrong".into(),
            worker_id: WorkerId::new(),
            trust_domain: "local".into(),
            labels: json!({}),
            incarnation_id: WorkerIncarnationId::new(),
            daemon_version: "test".into(),
            host_name: "host".into(),
            network_zone: None,
            capabilities: json!({}),
            capacity: 1,
            protocol_version: agentd_core::ports::WORKER_PROTOCOL_VERSION,
        })
        .await
        .expect_err("invalid proof");
    assert!(error.to_string().contains("authentication failed"));
}

#[tokio::test]
async fn empty_rotation_proof_set_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let fleet = SqliteWorkerFleet::new(store.pool().clone()).with_auth_proofs(Vec::<String>::new());
    let error = fleet
        .register(&WorkerFleetRegisterRequest {
            auth_proof: String::new(),
            worker_id: WorkerId::new(),
            trust_domain: "local".into(),
            labels: json!({}),
            incarnation_id: WorkerIncarnationId::new(),
            daemon_version: "test".into(),
            host_name: "host".into(),
            network_zone: None,
            capabilities: json!({}),
            capacity: 1,
            protocol_version: agentd_core::ports::WORKER_PROTOCOL_VERSION,
        })
        .await
        .expect_err("empty configured proof set must reject");
    assert!(error.to_string().contains("authentication failed"));
}

#[tokio::test]
async fn pull_routes_through_durable_queue_and_replays_by_request_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let proof = "fleet-secret".to_string();
    let fleet = SqliteWorkerFleet::new(store.pool().clone()).with_auth_proof(proof.clone());

    let run_id = RunId::new();
    run_repo::insert_run(store.pool(), &run_id, "workflow-sha")
        .await
        .expect("run");
    task_repo::insert_task_run(store.pool(), &run_id, &NodeId::parsed("impl"))
        .await
        .expect("task");

    let worker_id = WorkerId::new();
    worker_repo::create_worker(
        store.pool(),
        worker_repo::WorkerCreate {
            id: worker_id.clone(),
            trust_domain: "local".into(),
            labels: json!({}),
        },
    )
    .await
    .expect("worker");
    let incarnation_id = WorkerIncarnationId::new();
    worker_repo::register_incarnation(
        store.pool(),
        &worker_id,
        worker_repo::WorkerRegistration {
            id: incarnation_id.clone(),
            daemon_version: "test".into(),
            host_name: "host".into(),
            network_zone: None,
            capabilities: json!({}),
            capacity: 1,
        },
    )
    .await
    .expect("incarnation");

    let request = WorkerFleetPullRequest {
        auth_proof: proof.clone(),
        worker_incarnation_id: incarnation_id.clone(),
        observed_at: 20,
        expires_at: 80,
        request_id: Some("pull-1".to_string()),
    };
    let first = fleet.pull(&request).await.expect("pull").expect("grant");

    // The queue row is the authority now.
    let (status, lease): (String, Option<String>) = sqlx::query_as(
        "SELECT status, current_lease_id FROM execution_task_queue WHERE execution_task_id = ?",
    )
    .bind(first.execution_task_id.as_str())
    .fetch_one(store.pool())
    .await
    .expect("queue row");
    assert_eq!(status, "leased");
    assert_eq!(lease.as_deref(), Some(first.lease_id.as_str()));

    // Same request_id replays the same grant instead of erroring.
    let replay = fleet.pull(&request).await.expect("replay").expect("grant");
    assert_eq!(replay.lease_id, first.lease_id);
}

#[tokio::test]
async fn register_incarnation_persists_declared_capacity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let worker_id = WorkerId::new();
    worker_repo::create_worker(
        store.pool(),
        WorkerCreate {
            id: worker_id.clone(),
            trust_domain: "corp-coding".to_string(),
            labels: serde_json::json!({}),
        },
    )
    .await
    .expect("worker");
    let incarnation_id = WorkerIncarnationId::new();
    worker_repo::register_incarnation(
        store.pool(),
        &worker_id,
        WorkerRegistration {
            id: incarnation_id.clone(),
            daemon_version: "0.0.0-test".to_string(),
            host_name: "host-a".to_string(),
            network_zone: Some("dev".to_string()),
            capabilities: serde_json::json!({"runtime": ["codex"]}),
            capacity: 4,
        },
    )
    .await
    .expect("incarnation");
    let record = worker_repo::get_incarnation(store.pool(), &incarnation_id)
        .await
        .expect("read")
        .expect("incarnation exists");
    assert_eq!(record.capacity, 4);
    assert_eq!(record.network_zone.as_deref(), Some("dev"));
}
