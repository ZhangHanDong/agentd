use agentd_core::ports::{
    DurableSchedulerError, DurableSchedulerPort, SchedulerAcquireRequest, SchedulerEnqueueRequest,
    TaskLeaseCloseRequest, TaskLeasePort,
};
use agentd_core::types::{NodeId, RunId, SchedulerQueueStatus, TaskRunId, WorkerIncarnationId};
use agentd_store::durable_scheduler::SqliteDurableScheduler;
use agentd_store::task_lease_control_plane::SqliteTaskLeaseControlPlane;
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use agentd_store::{SqliteStore, run_repo, task_repo};
use serde_json::json;

struct Fixture {
    store: SqliteStore,
    _dir: tempfile::TempDir,
    task_id: TaskRunId,
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

fn enqueue_request(
    fixture: &Fixture,
    request_id: &str,
    available_at: i64,
) -> SchedulerEnqueueRequest {
    SchedulerEnqueueRequest {
        request_id: request_id.to_string(),
        execution_task_id: fixture.task_id.clone(),
        max_attempts: 3,
        available_at,
        enqueued_at: available_at,
    }
}

fn scheduler_for(fixture: &Fixture) -> SqliteDurableScheduler {
    SqliteDurableScheduler::new(fixture.store.pool().clone())
}

fn acquire_request(
    fixture: &Fixture,
    request_id: &str,
    observed_at: i64,
    expires_at: i64,
) -> SchedulerAcquireRequest {
    SchedulerAcquireRequest {
        request_id: request_id.to_string(),
        worker_incarnation_id: fixture.incarnation_id.clone(),
        observed_at,
        expires_at,
    }
}

#[tokio::test]
async fn enqueue_creates_a_queued_row_and_replays_identically() {
    let fixture = fixture().await;
    let scheduler = scheduler_for(&fixture);
    let request = enqueue_request(&fixture, "rq-1", 10);

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
    let scheduler = scheduler_for(&fixture);
    let request = enqueue_request(&fixture, "rq-1", 10);
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
    let scheduler = scheduler_for(&fixture);
    scheduler
        .enqueue(&enqueue_request(&fixture, "rq-1", 10))
        .await
        .expect("first enqueue");
    let error = scheduler
        .enqueue(&enqueue_request(&fixture, "rq-2", 11))
        .await
        .expect_err("second open row for the same task must conflict");
    assert!(matches!(error, DurableSchedulerError::Conflict(_)));
}

#[tokio::test]
async fn explain_reports_queue_row_and_absent_lease() {
    let fixture = fixture().await;
    let scheduler = scheduler_for(&fixture);
    assert!(
        scheduler
            .explain_task(&fixture.task_id)
            .await
            .expect("explain")
            .is_none()
    );
    scheduler
        .enqueue(&enqueue_request(&fixture, "rq-1", 10))
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

#[tokio::test]
async fn acquire_grants_lease_transitions_queue_and_appends_outbox() {
    let fixture = fixture().await;
    let scheduler = SqliteDurableScheduler::new(fixture.store.pool().clone());
    scheduler
        .enqueue(&enqueue_request(&fixture, "rq-1", 10))
        .await
        .expect("enqueue");

    let grant = scheduler
        .acquire(&SchedulerAcquireRequest {
            request_id: "acq-1".to_string(),
            worker_incarnation_id: fixture.incarnation_id.clone(),
            observed_at: 20,
            expires_at: 80,
        })
        .await
        .expect("acquire")
        .expect("eligible work");
    assert_eq!(grant.execution_task_id, fixture.task_id);

    let explanation = scheduler
        .explain_task(&fixture.task_id)
        .await
        .expect("explain")
        .expect("row");
    assert_eq!(explanation.queue.status, SchedulerQueueStatus::Leased);
    assert_eq!(
        explanation
            .queue
            .current_lease_id
            .as_ref()
            .map(|l| l.as_str().to_owned()),
        Some(grant.lease_id.as_str().to_owned())
    );
    assert!(explanation.active_lease.is_some());

    let (kind, task_id): (String, String) = sqlx::query_as(
        "SELECT kind, task_id FROM execution_scheduler_outbox ORDER BY seq DESC LIMIT 1",
    )
    .fetch_one(fixture.store.pool())
    .await
    .expect("outbox row");
    assert_eq!(kind, "lease_granted");
    assert_eq!(task_id, fixture.task_id.as_str());
}

#[tokio::test]
async fn acquire_replays_identical_grant_for_same_request_id() {
    let fixture = fixture().await;
    let scheduler = SqliteDurableScheduler::new(fixture.store.pool().clone());
    scheduler
        .enqueue(&enqueue_request(&fixture, "rq-1", 10))
        .await
        .expect("enqueue");
    let request = SchedulerAcquireRequest {
        request_id: "acq-1".to_string(),
        worker_incarnation_id: fixture.incarnation_id.clone(),
        observed_at: 20,
        expires_at: 80,
    };
    let first = scheduler
        .acquire(&request)
        .await
        .expect("acquire")
        .expect("grant");
    let replay = scheduler
        .acquire(&request)
        .await
        .expect("replay")
        .expect("grant");
    assert_eq!(
        first.lease_id, replay.lease_id,
        "replay returns the same lease"
    );
    let outbox_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_scheduler_outbox")
        .fetch_one(fixture.store.pool())
        .await
        .expect("count");
    assert_eq!(outbox_count, 1, "replay must not append a second event");
}

#[tokio::test]
async fn acquire_returns_none_when_nothing_eligible() {
    let fixture = fixture().await;
    let scheduler = SqliteDurableScheduler::new(fixture.store.pool().clone());
    // Nothing enqueued.
    assert!(
        scheduler
            .acquire(&SchedulerAcquireRequest {
                request_id: "acq-none".to_string(),
                worker_incarnation_id: fixture.incarnation_id.clone(),
                observed_at: 20,
                expires_at: 80,
            })
            .await
            .expect("acquire")
            .is_none()
    );
    // Enqueued but not yet available.
    scheduler
        .enqueue(&enqueue_request(&fixture, "rq-1", 1_000))
        .await
        .expect("enqueue");
    assert!(
        scheduler
            .acquire(&SchedulerAcquireRequest {
                request_id: "acq-early".to_string(),
                worker_incarnation_id: fixture.incarnation_id.clone(),
                observed_at: 20,
                expires_at: 80,
            })
            .await
            .expect("acquire")
            .is_none()
    );
}

#[tokio::test]
async fn concurrent_acquire_grants_exactly_one_winner() {
    let fixture = fixture().await;
    scheduler_for(&fixture)
        .enqueue(&enqueue_request(&fixture, "rq-1", 10))
        .await
        .expect("enqueue");
    let mut handles = Vec::new();
    for index in 0..4 {
        let pool = fixture.store.pool().clone();
        let incarnation = fixture.incarnation_id.clone();
        handles.push(tokio::spawn(async move {
            SqliteDurableScheduler::new(pool)
                .acquire(&SchedulerAcquireRequest {
                    request_id: format!("acq-{index}"),
                    worker_incarnation_id: incarnation,
                    observed_at: 20,
                    expires_at: 80,
                })
                .await
        }));
    }
    let mut grants = 0;
    for handle in handles {
        if handle.await.expect("join").expect("acquire").is_some() {
            grants += 1;
        }
    }
    assert_eq!(grants, 1, "exactly one concurrent acquirer wins");
}

#[tokio::test]
async fn reconcile_completes_row_when_lease_released() {
    let fixture = fixture().await;
    let scheduler = scheduler_for(&fixture);
    scheduler
        .enqueue(&enqueue_request(&fixture, "rq-1", 10))
        .await
        .expect("enqueue");
    let grant = scheduler
        .acquire(&acquire_request(&fixture, "acq-1", 20, 80))
        .await
        .expect("acquire")
        .expect("grant");
    let lease_plane = SqliteTaskLeaseControlPlane::new(fixture.store.pool().clone());
    lease_plane
        .release(&TaskLeaseCloseRequest {
            claim: grant.claim(),
            observed_at: 30,
            reason: "done".to_string(),
        })
        .await
        .expect("release");

    let changed = scheduler.reconcile(31).await.expect("reconcile");
    assert_eq!(changed, 1);
    let explanation = scheduler
        .explain_task(&fixture.task_id)
        .await
        .expect("explain")
        .expect("row");
    assert_eq!(explanation.queue.status, SchedulerQueueStatus::Completed);
}

#[tokio::test]
async fn reconcile_requeues_expired_lease_until_dead_letter() {
    let fixture = fixture().await;
    let scheduler = scheduler_for(&fixture);
    // max_attempts = 2: first expiry requeues, second dead-letters.
    let mut enqueue = enqueue_request(&fixture, "rq-1", 10);
    enqueue.max_attempts = 2;
    scheduler.enqueue(&enqueue).await.expect("enqueue");
    let lease_plane = SqliteTaskLeaseControlPlane::new(fixture.store.pool().clone());

    // Attempt 1: acquire then let it expire.
    scheduler
        .acquire(&acquire_request(&fixture, "acq-1", 20, 25))
        .await
        .expect("acquire")
        .expect("grant");
    lease_plane.expire_due(30).await.expect("expire");
    let changed = scheduler.reconcile(30).await.expect("reconcile");
    assert_eq!(changed, 1);
    let explanation = scheduler
        .explain_task(&fixture.task_id)
        .await
        .expect("explain")
        .expect("row");
    assert_eq!(
        explanation.queue.status,
        SchedulerQueueStatus::Queued,
        "first expiry requeues"
    );
    assert_eq!(explanation.queue.attempts, 1);

    // Attempt 2: acquire again, expire again -> dead letter.
    scheduler
        .acquire(&acquire_request(&fixture, "acq-2", 40, 45))
        .await
        .expect("acquire")
        .expect("grant");
    lease_plane.expire_due(50).await.expect("expire");
    scheduler.reconcile(50).await.expect("reconcile");
    let explanation = scheduler
        .explain_task(&fixture.task_id)
        .await
        .expect("explain")
        .expect("row");
    assert_eq!(explanation.queue.status, SchedulerQueueStatus::DeadLetter);
    assert!(
        explanation
            .queue
            .last_reason
            .as_deref()
            .unwrap_or("")
            .contains("expired")
    );
}
