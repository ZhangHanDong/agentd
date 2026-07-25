use agentd_core::ports::{DurableSchedulerError, DurableSchedulerPort, SchedulerEnqueueRequest};
use agentd_core::types::{NodeId, RunId, SchedulerQueueStatus, TaskRunId, WorkerIncarnationId};
use agentd_store::durable_scheduler::SqliteDurableScheduler;
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use agentd_store::{SqliteStore, run_repo, task_repo};
use serde_json::json;

struct Fixture {
    store: SqliteStore,
    _dir: tempfile::TempDir,
    task_id: TaskRunId,
    // Bound for fixture parity with enterprise_task_leases.rs; not read by
    // this file's queue-only tests (no lease dispatch here).
    #[allow(dead_code)]
    incarnation_id: WorkerIncarnationId,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let run_id = RunId::new();
    run_repo::insert_run(store.pool(), &run_id, "workflow-sha")
        .await
        .expect("run");
    let task_id = task_repo::insert_task_run(store.pool(), &run_id, &NodeId::parsed("impl"))
        .await
        .expect("task");
    let worker_id = agentd_core::types::WorkerId::new();
    worker_repo::create_worker(
        store.pool(),
        WorkerCreate {
            id: worker_id.clone(),
            trust_domain: "corp-coding".to_string(),
            labels: json!({"team": "runtime"}),
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
            daemon_version: "0.0.0-p270".to_string(),
            host_name: "host-a".to_string(),
            network_zone: Some("dev".to_string()),
            capabilities: json!({"runtime": ["codex"]}),
        },
    )
    .await
    .expect("incarnation");
    Fixture {
        store,
        _dir: dir,
        task_id,
        incarnation_id,
    }
}

#[tokio::test]
async fn enqueue_creates_a_queued_row_and_replays_identically() {
    let fixture = fixture().await;
    let scheduler = SqliteDurableScheduler::new(fixture.store.pool().clone());
    let request = SchedulerEnqueueRequest {
        request_id: "rq-1".to_string(),
        execution_task_id: fixture.task_id.clone(),
        max_attempts: 3,
        available_at: 10,
        enqueued_at: 10,
    };

    let first = scheduler.enqueue(&request).await.expect("enqueue");
    assert_eq!(first.status, SchedulerQueueStatus::Queued);
    assert_eq!(first.attempts, 0);
    assert_eq!(first.execution_task_id, fixture.task_id);

    let replay = scheduler.enqueue(&request).await.expect("replay");
    assert_eq!(replay, first, "same request_id replays the identical row");
}

#[tokio::test]
async fn enqueue_conflicts_on_same_request_with_different_payload() {
    let fixture = fixture().await;
    let scheduler = SqliteDurableScheduler::new(fixture.store.pool().clone());
    let request = SchedulerEnqueueRequest {
        request_id: "rq-1".to_string(),
        execution_task_id: fixture.task_id.clone(),
        max_attempts: 3,
        available_at: 10,
        enqueued_at: 10,
    };
    scheduler.enqueue(&request).await.expect("enqueue");

    let mut changed = request.clone();
    changed.max_attempts = 5;
    let error = scheduler
        .enqueue(&changed)
        .await
        .expect_err("changed payload must conflict");
    assert!(matches!(error, DurableSchedulerError::Conflict(_)));
}

#[tokio::test]
async fn enqueue_rejects_second_open_row_for_same_task() {
    let fixture = fixture().await;
    let scheduler = SqliteDurableScheduler::new(fixture.store.pool().clone());
    scheduler
        .enqueue(&SchedulerEnqueueRequest {
            request_id: "rq-1".to_string(),
            execution_task_id: fixture.task_id.clone(),
            max_attempts: 3,
            available_at: 10,
            enqueued_at: 10,
        })
        .await
        .expect("first enqueue");
    let error = scheduler
        .enqueue(&SchedulerEnqueueRequest {
            request_id: "rq-2".to_string(),
            execution_task_id: fixture.task_id.clone(),
            max_attempts: 3,
            available_at: 11,
            enqueued_at: 11,
        })
        .await
        .expect_err("second open row for the same task must conflict");
    assert!(matches!(error, DurableSchedulerError::Conflict(_)));
}

#[tokio::test]
async fn explain_reports_queue_row_and_absent_lease() {
    let fixture = fixture().await;
    let scheduler = SqliteDurableScheduler::new(fixture.store.pool().clone());
    assert!(
        scheduler
            .explain_task(&fixture.task_id)
            .await
            .expect("explain")
            .is_none()
    );
    scheduler
        .enqueue(&SchedulerEnqueueRequest {
            request_id: "rq-1".to_string(),
            execution_task_id: fixture.task_id.clone(),
            max_attempts: 3,
            available_at: 10,
            enqueued_at: 10,
        })
        .await
        .expect("enqueue");
    let explanation = scheduler
        .explain_task(&fixture.task_id)
        .await
        .expect("explain")
        .expect("queued task explains");
    assert_eq!(explanation.queue.status, SchedulerQueueStatus::Queued);
    assert!(explanation.active_lease.is_none());
}
