use agentd_core::ports::{
    WorkerFleetDrainRequest, WorkerFleetHeartbeat, WorkerFleetHeartbeatResult, WorkerFleetPort,
    WorkerFleetPullRequest, WorkerFleetRegisterRequest,
};
use agentd_core::types::{NodeId, RunId, WorkerId, WorkerIncarnationId, WorkerStatus};
use agentd_store::SqliteStore;
use agentd_store::worker_fleet::SqliteWorkerFleet;
use agentd_store::worker_repo;
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
            worker_id: worker_id.clone(),
            trust_domain: "corp".into(),
            labels: json!({"region": "cn-east"}),
            incarnation_id: first_incarnation.clone(),
            daemon_version: "test".into(),
            host_name: "host-a".into(),
            network_zone: Some("dev".into()),
            capabilities: json!({"runtime": ["native"]}),
        })
        .await
        .expect("register");
    assert!(matches!(
        fleet
            .heartbeat(&WorkerFleetHeartbeat {
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
            worker_id: worker_id.clone(),
            trust_domain: "corp".into(),
            labels: json!({}),
            incarnation_id: second_incarnation,
            daemon_version: "test".into(),
            host_name: "host-b".into(),
            network_zone: None,
            capabilities: json!({"runtime": ["native"]}),
        })
        .await
        .expect("re-register");
    assert_eq!(
        fleet
            .heartbeat(&WorkerFleetHeartbeat {
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
            worker_id: worker_id.clone(),
            trust_domain: "local".into(),
            labels: json!({}),
            incarnation_id: incarnation_id.clone(),
            daemon_version: "test".into(),
            host_name: "host".into(),
            network_zone: None,
            capabilities: json!({}),
        })
        .await
        .expect("register");

    fleet
        .set_drain(&WorkerFleetDrainRequest {
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
            worker_id: worker_id.clone(),
            trust_domain: "local".into(),
            labels: json!({}),
            incarnation_id,
            daemon_version: "test".into(),
            host_name: "host".into(),
            network_zone: None,
            capabilities: json!({}),
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
            worker_id,
            trust_domain: "local".into(),
            labels: json!({}),
            incarnation_id: incarnation_id.clone(),
            daemon_version: "test".into(),
            host_name: "host".into(),
            network_zone: None,
            capabilities: json!({}),
        })
        .await
        .expect("register");
    let grant = fleet
        .pull(&WorkerFleetPullRequest {
            worker_incarnation_id: incarnation_id,
            observed_at: 10,
            expires_at: 20,
        })
        .await
        .expect("pull")
        .expect("queued task");
    assert_eq!(grant.execution_task_id, task_id);
}
