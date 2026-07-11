use agentd_core::ports::{
    TaskLeaseCloseRequest, TaskLeaseDispatchRequest, TaskLeaseError, TaskLeasePort,
    TaskLeaseRejectionReason, TaskLeaseRenewRequest,
};
use agentd_core::types::{
    FencingToken, LeaseStatus, NodeId, RunId, TaskLeaseClaim, TaskRunId, WorkerId,
    WorkerIncarnationId, WorkerStatus,
};
use agentd_store::task_lease_control_plane::SqliteTaskLeaseControlPlane;
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use agentd_store::{SqliteStore, run_repo, task_repo};
use serde_json::json;
use sqlx::Row;

struct Fixture {
    store: SqliteStore,
    _dir: tempfile::TempDir,
    task_id: TaskRunId,
    worker_id: WorkerId,
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
    let worker_id = WorkerId::new();
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
        worker_id,
        incarnation_id,
    }
}

fn dispatch(
    task_id: TaskRunId,
    incarnation_id: WorkerIncarnationId,
    observed_at: i64,
    expires_at: i64,
) -> TaskLeaseDispatchRequest {
    TaskLeaseDispatchRequest {
        execution_task_id: task_id,
        worker_incarnation_id: incarnation_id,
        observed_at,
        expires_at,
    }
}

fn close(claim: TaskLeaseClaim, observed_at: i64, reason: &str) -> TaskLeaseCloseRequest {
    TaskLeaseCloseRequest {
        claim,
        observed_at,
        reason: reason.to_string(),
    }
}

fn assert_rejected(error: &TaskLeaseError, expected: TaskLeaseRejectionReason) {
    assert_eq!(error.rejection_reason(), Some(expected), "got {error:?}");
}

#[tokio::test]
async fn dispatch_binds_current_worker_and_allocates_first_fencing_token() {
    let fixture = fixture().await;
    let control_plane = SqliteTaskLeaseControlPlane::new(fixture.store.pool().clone());
    let grant = control_plane
        .dispatch(&dispatch(
            fixture.task_id.clone(),
            fixture.incarnation_id.clone(),
            100,
            200,
        ))
        .await
        .expect("first dispatch");
    assert_eq!(grant.execution_task_id, fixture.task_id);
    assert_eq!(grant.worker_incarnation_id, fixture.incarnation_id);
    assert_eq!(grant.fencing_token.value(), 1);
    assert_eq!(grant.status, LeaseStatus::Active);
    assert_eq!(grant.acquired_at, 100);
    assert_eq!(grant.expires_at, 200);
    assert!(grant.lease_id.as_str().starts_with("ls_"));
    grant.lease_id.as_str()[3..]
        .parse::<ulid::Ulid>()
        .expect("lease ULID");

    let head = sqlx::query(
        "SELECT last_fencing_token, current_lease_id \
         FROM execution_task_lease_heads WHERE execution_task_id = ?",
    )
    .bind(fixture.task_id.as_str())
    .fetch_one(fixture.store.pool())
    .await
    .expect("lease head");
    assert_eq!(head.get::<i64, _>("last_fencing_token"), 1);
    assert_eq!(
        head.get::<Option<String>, _>("current_lease_id"),
        Some(grant.lease_id.to_string())
    );

    let unknown = control_plane
        .dispatch(&dispatch(
            TaskRunId::new(),
            fixture.incarnation_id.clone(),
            100,
            200,
        ))
        .await
        .expect_err("unknown task");
    assert!(matches!(unknown, TaskLeaseError::NotFound(_)));

    let malformed = control_plane
        .dispatch(&dispatch(
            TaskRunId::from_string("ticket-p270"),
            fixture.incarnation_id.clone(),
            100,
            200,
        ))
        .await
        .expect_err("malformed canonical task id");
    assert!(matches!(malformed, TaskLeaseError::Invalid(_)));

    let run_id = RunId::new();
    run_repo::insert_run(fixture.store.pool(), &run_id, "finished-workflow")
        .await
        .expect("finished run");
    let finished_task =
        task_repo::insert_task_run(fixture.store.pool(), &run_id, &NodeId::parsed("done"))
            .await
            .expect("finished task");
    task_repo::complete_task_run(fixture.store.pool(), &finished_task)
        .await
        .expect("finish task");
    let finished = control_plane
        .dispatch(&dispatch(
            finished_task,
            fixture.incarnation_id.clone(),
            100,
            200,
        ))
        .await
        .expect_err("finished task cannot dispatch");
    assert!(matches!(finished, TaskLeaseError::Conflict(_)));

    worker_repo::transition_worker_status(
        fixture.store.pool(),
        &fixture.worker_id,
        WorkerStatus::Draining,
    )
    .await
    .expect("drain worker");
    let draining = control_plane
        .dispatch(&dispatch(fixture.task_id, fixture.incarnation_id, 201, 300))
        .await
        .expect_err("draining worker cannot receive new dispatch");
    assert!(matches!(draining, TaskLeaseError::Conflict(_)));
}

#[tokio::test]
async fn active_conflict_and_reacquisition_allocate_new_monotonic_grants() {
    let fixture = fixture().await;
    let control_plane = SqliteTaskLeaseControlPlane::new(fixture.store.pool().clone());
    let first = control_plane
        .dispatch(&dispatch(
            fixture.task_id.clone(),
            fixture.incarnation_id.clone(),
            100,
            110,
        ))
        .await
        .expect("first dispatch");

    let conflict = control_plane
        .dispatch(&dispatch(
            fixture.task_id.clone(),
            fixture.incarnation_id.clone(),
            105,
            150,
        ))
        .await
        .expect_err("unexpired active lease conflicts");
    assert!(matches!(conflict, TaskLeaseError::Conflict(_)));

    let unchanged: (String, i64, String) = sqlx::query_as(
        "SELECT id, fencing_token, status FROM execution_task_leases \
         WHERE execution_task_id = ?",
    )
    .bind(fixture.task_id.as_str())
    .fetch_one(fixture.store.pool())
    .await
    .expect("unchanged active lease");
    assert_eq!(
        unchanged,
        (first.lease_id.to_string(), 1, "active".to_string())
    );

    let second = control_plane
        .dispatch(&dispatch(
            fixture.task_id.clone(),
            fixture.incarnation_id,
            110,
            200,
        ))
        .await
        .expect("dispatch after exact expiry");
    assert_ne!(second.lease_id, first.lease_id);
    assert!(second.fencing_token > first.fencing_token);
    assert_eq!(second.fencing_token.value(), 2);

    let first_status: String =
        sqlx::query_scalar("SELECT status FROM execution_task_leases WHERE id = ?")
            .bind(first.lease_id.as_str())
            .fetch_one(fixture.store.pool())
            .await
            .expect("first terminal status");
    assert_eq!(first_status, "expired");
    let active_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_task_leases \
         WHERE execution_task_id = ? AND status = 'active'",
    )
    .bind(fixture.task_id.as_str())
    .fetch_one(fixture.store.pool())
    .await
    .expect("active count");
    assert_eq!(active_count, 1);
}

#[tokio::test]
async fn renew_release_and_cancel_require_exact_current_claim() {
    let fixture = fixture().await;
    let control_plane = SqliteTaskLeaseControlPlane::new(fixture.store.pool().clone());
    let first = control_plane
        .dispatch(&dispatch(
            fixture.task_id.clone(),
            fixture.incarnation_id.clone(),
            100,
            200,
        ))
        .await
        .expect("first dispatch");

    let renewed = control_plane
        .renew(&TaskLeaseRenewRequest {
            claim: first.claim(),
            observed_at: 150,
            expires_at: 250,
        })
        .await
        .expect("exact renewal");
    assert_eq!(renewed.status, LeaseStatus::Active);
    assert_eq!(renewed.expires_at, 250);
    assert_eq!(renewed.renewed_at, Some(150));
    assert_eq!(renewed.record_version, 2);

    let mut stale_token = renewed.claim();
    stale_token.fencing_token = FencingToken::new(2).expect("token");
    let stale_error = control_plane
        .renew(&TaskLeaseRenewRequest {
            claim: stale_token,
            observed_at: 160,
            expires_at: 260,
        })
        .await
        .expect_err("mismatched token");
    assert_rejected(&stale_error, TaskLeaseRejectionReason::StaleFencingToken);

    let mut wrong_task = renewed.claim();
    wrong_task.execution_task_id = TaskRunId::new();
    let wrong_task_error = control_plane
        .release(&close(wrong_task, 170, "worker_done"))
        .await
        .expect_err("mismatched task");
    assert_rejected(&wrong_task_error, TaskLeaseRejectionReason::ClaimMismatch);

    let released = control_plane
        .release(&close(renewed.claim(), 180, "worker_done"))
        .await
        .expect("exact release");
    assert_eq!(released.status, LeaseStatus::Released);
    assert_eq!(released.terminal_at, Some(180));
    assert_eq!(released.terminal_reason.as_deref(), Some("worker_done"));
    assert_eq!(released.record_version, 3);
    let current_after_release: Option<String> = sqlx::query_scalar(
        "SELECT current_lease_id FROM execution_task_lease_heads WHERE execution_task_id = ?",
    )
    .bind(fixture.task_id.as_str())
    .fetch_one(fixture.store.pool())
    .await
    .expect("head after release");
    assert!(current_after_release.is_none());

    let terminal_renewal = control_plane
        .renew(&TaskLeaseRenewRequest {
            claim: released.claim(),
            observed_at: 181,
            expires_at: 300,
        })
        .await
        .expect_err("terminal renewal");
    assert_rejected(&terminal_renewal, TaskLeaseRejectionReason::TerminalLease);

    let second = control_plane
        .dispatch(&dispatch(
            fixture.task_id.clone(),
            fixture.incarnation_id,
            181,
            300,
        ))
        .await
        .expect("redispatch after release");
    assert_eq!(second.fencing_token.value(), 2);
    let cancelled = control_plane
        .cancel(&close(second.claim(), 190, "operator_cancelled"))
        .await
        .expect("exact cancellation");
    assert_eq!(cancelled.status, LeaseStatus::Cancelled);
    assert_eq!(
        cancelled.terminal_reason.as_deref(),
        Some("operator_cancelled")
    );
    let terminal_release = control_plane
        .release(&close(cancelled.claim(), 191, "late_release"))
        .await
        .expect_err("terminal release");
    assert_rejected(&terminal_release, TaskLeaseRejectionReason::TerminalLease);
}

#[tokio::test]
async fn stale_terminal_and_expired_claims_are_rejected() {
    let fixture = fixture().await;
    let control_plane = SqliteTaskLeaseControlPlane::new(fixture.store.pool().clone());
    let first = control_plane
        .dispatch(&dispatch(
            fixture.task_id.clone(),
            fixture.incarnation_id.clone(),
            100,
            110,
        ))
        .await
        .expect("first dispatch");
    let second = control_plane
        .dispatch(&dispatch(
            fixture.task_id.clone(),
            fixture.incarnation_id.clone(),
            110,
            200,
        ))
        .await
        .expect("second dispatch");

    let old_token = control_plane
        .validate_claim(&first.claim(), 120)
        .await
        .expect_err("old token cannot authorize");
    assert_rejected(&old_token, TaskLeaseRejectionReason::StaleFencingToken);

    let released = control_plane
        .release(&close(second.claim(), 130, "worker_done"))
        .await
        .expect("release second");
    let terminal = control_plane
        .validate_claim(&released.claim(), 131)
        .await
        .expect_err("terminal lease cannot authorize");
    assert_rejected(&terminal, TaskLeaseRejectionReason::TerminalLease);

    let third = control_plane
        .dispatch(&dispatch(
            fixture.task_id.clone(),
            fixture.incarnation_id.clone(),
            200,
            210,
        ))
        .await
        .expect("third dispatch");
    let elapsed = control_plane
        .validate_claim(&third.claim(), 210)
        .await
        .expect_err("elapsed lease cannot authorize");
    assert_rejected(&elapsed, TaskLeaseRejectionReason::LeaseExpired);
    let third_status: String =
        sqlx::query_scalar("SELECT status FROM execution_task_leases WHERE id = ?")
            .bind(third.lease_id.as_str())
            .fetch_one(fixture.store.pool())
            .await
            .expect("third status");
    assert_eq!(third_status, "expired");

    let fourth = control_plane
        .dispatch(&dispatch(fixture.task_id, fixture.incarnation_id, 220, 230))
        .await
        .expect("fourth dispatch");
    assert_eq!(
        control_plane.expire_due(230).await.expect("expire sweep"),
        1
    );
    let fourth_status: String =
        sqlx::query_scalar("SELECT status FROM execution_task_leases WHERE id = ?")
            .bind(fourth.lease_id.as_str())
            .fetch_one(fixture.store.pool())
            .await
            .expect("fourth status");
    assert_eq!(fourth_status, "expired");
}

#[tokio::test]
async fn worker_reincarnation_supersedes_old_lease_before_new_dispatch() {
    let fixture = fixture().await;
    let control_plane = SqliteTaskLeaseControlPlane::new(fixture.store.pool().clone());
    let first = control_plane
        .dispatch(&dispatch(
            fixture.task_id.clone(),
            fixture.incarnation_id.clone(),
            100,
            200,
        ))
        .await
        .expect("first dispatch");

    let next_incarnation_id = WorkerIncarnationId::new();
    worker_repo::register_incarnation(
        fixture.store.pool(),
        &fixture.worker_id,
        WorkerRegistration {
            id: next_incarnation_id.clone(),
            daemon_version: "0.0.0-p270-restart".to_string(),
            host_name: "host-b".to_string(),
            network_zone: Some("dev".to_string()),
            capabilities: json!({"runtime": ["codex"]}),
        },
    )
    .await
    .expect("worker restart");

    let stale_worker = control_plane
        .validate_claim(&first.claim(), 110)
        .await
        .expect_err("superseded worker claim");
    assert_rejected(
        &stale_worker,
        TaskLeaseRejectionReason::StaleWorkerIncarnation,
    );

    let second = control_plane
        .dispatch(&dispatch(
            fixture.task_id.clone(),
            next_incarnation_id.clone(),
            120,
            220,
        ))
        .await
        .expect("dispatch to new incarnation");
    assert_ne!(second.lease_id, first.lease_id);
    assert_eq!(second.fencing_token.value(), 2);
    assert_eq!(second.worker_incarnation_id, next_incarnation_id);
    let first_status: String =
        sqlx::query_scalar("SELECT status FROM execution_task_leases WHERE id = ?")
            .bind(first.lease_id.as_str())
            .fetch_one(fixture.store.pool())
            .await
            .expect("first status");
    assert_eq!(first_status, "superseded");

    let stale_release = control_plane
        .release(&close(first.claim(), 121, "late_release"))
        .await
        .expect_err("old grant release");
    assert!(stale_release.rejection_reason().is_some());

    let forged_worker_claim = TaskLeaseClaim {
        worker_incarnation_id: fixture.incarnation_id,
        ..second.claim()
    };
    let forged_worker = control_plane
        .validate_claim(&forged_worker_claim, 130)
        .await
        .expect_err("old worker cannot claim new grant");
    assert_rejected(&forged_worker, TaskLeaseRejectionReason::ClaimMismatch);
}

#[tokio::test]
async fn concurrent_dispatch_has_one_active_lease_and_unique_token() {
    let fixture = fixture().await;
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));
    let first_port = SqliteTaskLeaseControlPlane::new(fixture.store.pool().clone());
    let second_port = first_port.clone();
    let first_request = dispatch(
        fixture.task_id.clone(),
        fixture.incarnation_id.clone(),
        100,
        200,
    );
    let second_request = first_request.clone();

    let first_barrier = barrier.clone();
    let first = tokio::spawn(async move {
        first_barrier.wait().await;
        first_port.dispatch(&first_request).await
    });
    let second_barrier = barrier.clone();
    let second = tokio::spawn(async move {
        second_barrier.wait().await;
        second_port.dispatch(&second_request).await
    });
    barrier.wait().await;

    let results = [
        first.await.expect("first join"),
        second.await.expect("second join"),
    ];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let errors = results
        .iter()
        .filter_map(|result| result.as_ref().err())
        .collect::<Vec<_>>();
    assert_eq!(errors.len(), 1);
    assert!(matches!(errors[0], TaskLeaseError::Conflict(_)));

    let active_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_task_leases \
         WHERE execution_task_id = ? AND status = 'active'",
    )
    .bind(fixture.task_id.as_str())
    .fetch_one(fixture.store.pool())
    .await
    .expect("active lease count");
    let distinct_tokens: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT fencing_token) FROM execution_task_leases \
         WHERE execution_task_id = ?",
    )
    .bind(fixture.task_id.as_str())
    .fetch_one(fixture.store.pool())
    .await
    .expect("distinct token count");
    let head: (i64, Option<String>) = sqlx::query_as(
        "SELECT last_fencing_token, current_lease_id \
         FROM execution_task_lease_heads WHERE execution_task_id = ?",
    )
    .bind(fixture.task_id.as_str())
    .fetch_one(fixture.store.pool())
    .await
    .expect("lease head");
    let active_id: String = sqlx::query_scalar(
        "SELECT id FROM execution_task_leases \
         WHERE execution_task_id = ? AND status = 'active'",
    )
    .bind(fixture.task_id.as_str())
    .fetch_one(fixture.store.pool())
    .await
    .expect("active lease id");
    assert_eq!(active_count, 1);
    assert_eq!(distinct_tokens, 1);
    assert_eq!(head, (1, Some(active_id)));
}
