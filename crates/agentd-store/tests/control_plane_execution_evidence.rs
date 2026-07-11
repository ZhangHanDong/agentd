use agentd_core::ports::{
    ArtifactIndexPort, ArtifactListRequest, AuditActorKind, AuditReadRequest,
    CertificationReferenceAppend, CertificationReferenceKind, CertificationReferencePort,
    ExecutionArtifactKind, ExecutionArtifactPublish, ExecutionAuditAppend, ExecutionAuditPort,
    ExecutionEvidenceError, ExecutionEvidenceLinks, ExecutionSnapshotLink, PageLimit,
    TaskLeaseCloseRequest, TaskLeaseDispatchRequest, TaskLeasePort, TaskLeaseRejectionReason,
    UsageLedgerPort, UsageMeasurement, UsageMetric, UsageReadRequest, UsageTotal,
    WorkerArtifactReport, WorkerUsageReport,
};
use agentd_core::types::{
    AgentProfileId, AuditEventId, ExecutionArtifactId, NodeId, RunId, RuntimeAttemptId,
    RuntimeSessionId, TaskRunId, WorkerId, WorkerIncarnationId,
};
use agentd_store::agent_profile_repo::{self, AgentProfileCreate};
use agentd_store::execution_evidence_control_plane::SqliteExecutionEvidenceControlPlane;
use agentd_store::runtime_session_repo::{
    self, ExecutionSnapshotRef, RuntimeAttemptCreate, RuntimeSessionCreate,
};
use agentd_store::task_lease_control_plane::SqliteTaskLeaseControlPlane;
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use agentd_store::{SqliteStore, run_repo, task_repo};
use serde_json::json;

struct Fixture {
    store: SqliteStore,
    _dir: tempfile::TempDir,
    run_id: RunId,
    task_id: TaskRunId,
    worker_id: WorkerId,
    incarnation_id: WorkerIncarnationId,
    links: ExecutionEvidenceLinks,
}

#[allow(clippy::too_many_lines)]
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
    let profile_id = AgentProfileId::new();
    agent_profile_repo::create_profile(
        store.pool(),
        AgentProfileCreate {
            id: profile_id.clone(),
            role: "implementer".to_string(),
            capability: Some("implementation".to_string()),
            runtime: "codex".to_string(),
            model: Some("gpt-5".to_string()),
            prompt_profile: Some("default".to_string()),
        },
    )
    .await
    .expect("profile");
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
            daemon_version: "0.0.0-p271".to_string(),
            host_name: "host-a".to_string(),
            network_zone: Some("dev".to_string()),
            capabilities: json!({"runtime": ["codex"]}),
        },
    )
    .await
    .expect("incarnation");
    let snapshot = ExecutionSnapshotLink {
        authority_key: "specify:corp".to_string(),
        resource_kind: "execution_snapshot".to_string(),
        resource_id: "snapshot-42".to_string(),
        resource_version: "7".to_string(),
        content_sha256: "a".repeat(64),
    };
    let session_id = RuntimeSessionId::new();
    runtime_session_repo::create_session(
        store.pool(),
        RuntimeSessionCreate {
            id: session_id.clone(),
            execution_task_id: task_id.clone(),
            agent_profile_id: profile_id,
            snapshot: ExecutionSnapshotRef {
                authority_key: snapshot.authority_key.clone(),
                resource_kind: snapshot.resource_kind.clone(),
                resource_id: snapshot.resource_id.clone(),
                resource_version: snapshot.resource_version.clone(),
                content_sha256: snapshot.content_sha256.clone(),
            },
        },
    )
    .await
    .expect("session");
    let attempt_id = RuntimeAttemptId::new();
    runtime_session_repo::start_attempt(
        store.pool(),
        &session_id,
        RuntimeAttemptCreate {
            id: attempt_id.clone(),
            worker_incarnation_id: incarnation_id.clone(),
            backend_target: Some("native://attempt".to_string()),
            session_name: None,
            pane_id: None,
            pid: Some(100),
            native_session_ref: Some("codex-resume".to_string()),
            workdir: Some("/tmp/worktree".to_string()),
        },
    )
    .await
    .expect("attempt");
    let links = ExecutionEvidenceLinks {
        execution_run_id: run_id.clone(),
        execution_task_id: Some(task_id.clone()),
        runtime_session_id: Some(session_id.clone()),
        runtime_attempt_id: Some(attempt_id.clone()),
        worker_incarnation_id: Some(incarnation_id.clone()),
        snapshot,
        target_repository_id: "repo_01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
        target_base_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
    };
    Fixture {
        store,
        _dir: dir,
        run_id,
        task_id,
        worker_id,
        incarnation_id,
        links,
    }
}

fn control_plane(
    fixture: &Fixture,
) -> SqliteExecutionEvidenceControlPlane<SqliteTaskLeaseControlPlane> {
    let lease = SqliteTaskLeaseControlPlane::new(fixture.store.pool().clone());
    SqliteExecutionEvidenceControlPlane::new(fixture.store.pool().clone(), lease)
}

fn artifact(id: &str, kind: ExecutionArtifactKind, fixture: &Fixture) -> ExecutionArtifactPublish {
    ExecutionArtifactPublish {
        id: ExecutionArtifactId::from_string(id),
        kind,
        content_sha256: "b".repeat(64),
        size_bytes: 42,
        media_type: "application/json".to_string(),
        storage_ref: format!("cas://{id}"),
        provenance: json!({"tool": "cargo-test", "version": 1}),
        links: fixture.links.clone(),
    }
}

fn audit(id: &str, key: &str, occurred_at: i64, fixture: &Fixture) -> ExecutionAuditAppend {
    ExecutionAuditAppend {
        id: AuditEventId::from_string(id),
        idempotency_scope: format!("run:{}", fixture.run_id),
        idempotency_key: key.to_string(),
        event_type: "execution.test".to_string(),
        actor_kind: AuditActorKind::ControlPlane,
        actor_ref: "agentd".to_string(),
        payload_sha256: "c".repeat(64),
        payload: json!({"key": key}),
        links: fixture.links.clone(),
        execution_artifact_id: None,
        occurred_at,
    }
}

fn usage(
    id: &str,
    key: &str,
    metric: UsageMetric,
    quantity: u64,
    fixture: &Fixture,
) -> UsageMeasurement {
    UsageMeasurement {
        id: AuditEventId::from_string(id),
        idempotency_scope: format!("run:{}", fixture.run_id),
        idempotency_key: key.to_string(),
        actor_kind: AuditActorKind::Worker,
        actor_ref: fixture.incarnation_id.to_string(),
        metric,
        quantity,
        provider: Some("openai".to_string()),
        model: Some("gpt-5".to_string()),
        links: fixture.links.clone(),
        measured_at: 150,
    }
}

#[tokio::test]
async fn artifact_index_publish_is_idempotent_and_lists_by_run() {
    let fixture = fixture().await;
    let control_plane = control_plane(&fixture);
    let first = artifact(
        "ar_01ARZ3NDEKTSV4RRFFQ69G5FAV",
        ExecutionArtifactKind::Plan,
        &fixture,
    );
    let first_record = control_plane
        .publish_artifact(&first)
        .await
        .expect("publish first");
    assert_eq!(
        control_plane
            .publish_artifact(&first)
            .await
            .expect("exact retry"),
        first_record
    );

    let mut changed = first.clone();
    changed.media_type = "text/plain".to_string();
    let changed_error = control_plane
        .publish_artifact(&changed)
        .await
        .expect_err("changed retry");
    assert!(matches!(changed_error, ExecutionEvidenceError::Conflict(_)));
    assert_eq!(
        control_plane
            .get_artifact(&first.id)
            .await
            .expect("get first"),
        Some(first_record.clone())
    );

    let second = artifact(
        "ar_01ARZ3NDEKTSV4RRFFQ69G5FAW",
        ExecutionArtifactKind::TestReport,
        &fixture,
    );
    control_plane
        .publish_artifact(&second)
        .await
        .expect("publish second");
    let page_one = control_plane
        .list_artifacts(&ArtifactListRequest {
            execution_run_id: fixture.run_id.clone(),
            cursor: None,
            limit: PageLimit::new(1).expect("limit"),
        })
        .await
        .expect("first page");
    assert_eq!(page_one.records.len(), 1);
    assert_eq!(page_one.records[0], first_record);
    let page_two = control_plane
        .list_artifacts(&ArtifactListRequest {
            execution_run_id: fixture.run_id,
            cursor: Some(page_one.next_cursor.expect("next cursor")),
            limit: PageLimit::new(1).expect("limit"),
        })
        .await
        .expect("second page");
    assert_eq!(page_two.records.len(), 1);
    assert_eq!(page_two.records[0].publish, second);
    assert!(page_two.next_cursor.is_none());
}

#[tokio::test]
async fn audit_log_append_and_bounded_replay_preserve_sequence() {
    let fixture = fixture().await;
    let control_plane = control_plane(&fixture);
    let first = audit("ae_01ARZ3NDEKTSV4RRFFQ69G5FAX", "event-1", 200, &fixture);
    let first_record = control_plane
        .append_audit(&first)
        .await
        .expect("append first");
    assert_eq!(
        control_plane
            .append_audit(&first)
            .await
            .expect("exact retry"),
        first_record
    );
    let mut changed = first.clone();
    changed.payload_sha256 = "d".repeat(64);
    changed.payload = json!({"changed": true});
    let changed_error = control_plane
        .append_audit(&changed)
        .await
        .expect_err("changed retry");
    assert!(matches!(changed_error, ExecutionEvidenceError::Conflict(_)));

    let second = audit("ae_01ARZ3NDEKTSV4RRFFQ69G5FAY", "event-2", 100, &fixture);
    let second_record = control_plane
        .append_audit(&second)
        .await
        .expect("append second");
    assert!(second_record.sequence > first_record.sequence);
    assert!(second_record.append.occurred_at < first_record.append.occurred_at);

    let page_one = control_plane
        .read_audit(&AuditReadRequest {
            execution_run_id: fixture.run_id.clone(),
            after_sequence: 0,
            limit: PageLimit::new(1).expect("limit"),
        })
        .await
        .expect("first audit page");
    assert_eq!(page_one.records, vec![first_record.clone()]);
    assert_eq!(page_one.next_after_sequence, Some(first_record.sequence));
    let page_two = control_plane
        .read_audit(&AuditReadRequest {
            execution_run_id: fixture.run_id,
            after_sequence: page_one.next_after_sequence.expect("next sequence"),
            limit: PageLimit::new(1).expect("limit"),
        })
        .await
        .expect("second audit page");
    assert_eq!(page_two.records, vec![second_record]);
    assert!(page_two.next_after_sequence.is_none());
}

#[tokio::test]
async fn certification_reference_port_records_external_refs_without_delivery_gate() {
    let fixture = fixture().await;
    let control_plane = control_plane(&fixture);
    let artifact_request = artifact(
        "ar_01ARZ3NDEKTSV4RRFFQ69G5FAZ",
        ExecutionArtifactKind::Commit,
        &fixture,
    );
    let original_artifact = control_plane
        .publish_artifact(&artifact_request)
        .await
        .expect("publish artifact");

    let mut expected = Vec::new();
    for (kind, external_ref) in [
        (CertificationReferenceKind::Request, "of-request-1"),
        (CertificationReferenceKind::Result, "of-result-1"),
        (CertificationReferenceKind::Signature, "of-signature-1"),
        (CertificationReferenceKind::Attestation, "of-attestation-1"),
    ] {
        let request = CertificationReferenceAppend {
            execution_artifact_id: artifact_request.id.clone(),
            authority_key: "openfab:prod".to_string(),
            kind,
            external_ref: external_ref.to_string(),
        };
        let record = control_plane
            .append_certification_reference(&request)
            .await
            .expect("append cert ref");
        assert_eq!(
            control_plane
                .append_certification_reference(&request)
                .await
                .expect("retry cert ref"),
            record
        );
        expected.push(record);
    }
    assert_eq!(
        control_plane
            .list_certification_references(&artifact_request.id)
            .await
            .expect("list refs"),
        expected
    );

    let changed = CertificationReferenceAppend {
        execution_artifact_id: artifact_request.id.clone(),
        authority_key: "openfab:prod".to_string(),
        kind: CertificationReferenceKind::Request,
        external_ref: "of-request-changed".to_string(),
    };
    assert!(matches!(
        control_plane
            .append_certification_reference(&changed)
            .await
            .expect_err("changed ref"),
        ExecutionEvidenceError::Conflict(_)
    ));

    let second_artifact = artifact(
        "ar_01ARZ3NDEKTSV4RRFFQ69G5FB0",
        ExecutionArtifactKind::TestReport,
        &fixture,
    );
    control_plane
        .publish_artifact(&second_artifact)
        .await
        .expect("publish second artifact");
    let reused = CertificationReferenceAppend {
        execution_artifact_id: second_artifact.id,
        authority_key: "openfab:prod".to_string(),
        kind: CertificationReferenceKind::Result,
        external_ref: "of-result-1".to_string(),
    };
    assert!(matches!(
        control_plane
            .append_certification_reference(&reused)
            .await
            .expect_err("cross-artifact ref reuse"),
        ExecutionEvidenceError::Conflict(_)
    ));
    assert_eq!(
        control_plane
            .get_artifact(&artifact_request.id)
            .await
            .expect("artifact after refs"),
        Some(original_artifact)
    );

    let usage_tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='execution_usage_records'",
    )
    .fetch_one(fixture.store.pool())
    .await
    .expect("usage table absence");
    assert_eq!(usage_tables, 0);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn usage_ledger_records_typed_audit_measurements_and_totals() {
    let fixture = fixture().await;
    let control_plane = control_plane(&fixture);
    let measurements = [
        usage(
            "ae_01ARZ3NDEKTSV4RRFFQ69G5FB1",
            "usage-input",
            UsageMetric::InputTokens,
            10,
            &fixture,
        ),
        usage(
            "ae_01ARZ3NDEKTSV4RRFFQ69G5FB2",
            "usage-cached",
            UsageMetric::CachedInputTokens,
            3,
            &fixture,
        ),
        usage(
            "ae_01ARZ3NDEKTSV4RRFFQ69G5FB3",
            "usage-output",
            UsageMetric::OutputTokens,
            5,
            &fixture,
        ),
        usage(
            "ae_01ARZ3NDEKTSV4RRFFQ69G5FB4",
            "usage-reasoning",
            UsageMetric::ReasoningTokens,
            2,
            &fixture,
        ),
        usage(
            "ae_01ARZ3NDEKTSV4RRFFQ69G5FB5",
            "usage-tools",
            UsageMetric::ToolCalls,
            1,
            &fixture,
        ),
        usage(
            "ae_01ARZ3NDEKTSV4RRFFQ69G5FB6",
            "usage-runtime",
            UsageMetric::RuntimeMilliseconds,
            100,
            &fixture,
        ),
        usage(
            "ae_01ARZ3NDEKTSV4RRFFQ69G5FB7",
            "usage-artifact",
            UsageMetric::ArtifactBytes,
            42,
            &fixture,
        ),
    ];
    let mut records = Vec::new();
    for measurement in &measurements {
        records.push(
            control_plane
                .record_usage(measurement)
                .await
                .expect("record usage"),
        );
    }
    assert_eq!(
        control_plane
            .record_usage(&measurements[0])
            .await
            .expect("exact usage retry"),
        records[0]
    );

    let mut changed = measurements[0].clone();
    changed.quantity = 11;
    let changed_error = control_plane
        .record_usage(&changed)
        .await
        .expect_err("changed usage retry");
    assert!(matches!(changed_error, ExecutionEvidenceError::Conflict(_)));

    let first_page = control_plane
        .read_usage(&UsageReadRequest {
            execution_run_id: fixture.run_id.clone(),
            after_sequence: 0,
            limit: PageLimit::new(3).expect("limit"),
        })
        .await
        .expect("usage page one");
    assert_eq!(first_page.records, records[..3]);
    let second_page = control_plane
        .read_usage(&UsageReadRequest {
            execution_run_id: fixture.run_id.clone(),
            after_sequence: first_page.next_after_sequence.expect("usage cursor"),
            limit: PageLimit::new(4).expect("limit"),
        })
        .await
        .expect("usage page two");
    assert_eq!(second_page.records, records[3..]);
    assert!(second_page.next_after_sequence.is_none());

    let totals = control_plane
        .usage_totals(&fixture.run_id)
        .await
        .expect("usage totals");
    assert_eq!(
        totals.totals,
        vec![
            UsageTotal {
                metric: UsageMetric::InputTokens,
                quantity: 10,
            },
            UsageTotal {
                metric: UsageMetric::CachedInputTokens,
                quantity: 3,
            },
            UsageTotal {
                metric: UsageMetric::OutputTokens,
                quantity: 5,
            },
            UsageTotal {
                metric: UsageMetric::ReasoningTokens,
                quantity: 2,
            },
            UsageTotal {
                metric: UsageMetric::ToolCalls,
                quantity: 1,
            },
            UsageTotal {
                metric: UsageMetric::RuntimeMilliseconds,
                quantity: 100,
            },
            UsageTotal {
                metric: UsageMetric::ArtifactBytes,
                quantity: 42,
            },
        ]
    );

    let usage_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_audit_events WHERE event_type='usage.measured'",
    )
    .fetch_one(fixture.store.pool())
    .await
    .expect("usage audit count");
    assert_eq!(usage_events, 7);
    let usage_tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='execution_usage_records'",
    )
    .fetch_one(fixture.store.pool())
    .await
    .expect("usage table absence");
    assert_eq!(usage_tables, 0);
}

#[tokio::test]
async fn worker_artifact_publish_requires_current_lease_and_audits_rejection() {
    let fixture = fixture().await;
    let lease_port = SqliteTaskLeaseControlPlane::new(fixture.store.pool().clone());
    let control_plane =
        SqliteExecutionEvidenceControlPlane::new(fixture.store.pool().clone(), lease_port.clone());
    let first_grant = lease_port
        .dispatch(&TaskLeaseDispatchRequest {
            execution_task_id: fixture.task_id.clone(),
            worker_incarnation_id: fixture.incarnation_id.clone(),
            observed_at: 100,
            expires_at: 200,
        })
        .await
        .expect("first lease");
    let artifact = artifact(
        "ar_01ARZ3NDEKTSV4RRFFQ69G5FB8",
        ExecutionArtifactKind::TestReport,
        &fixture,
    );
    let accepted = control_plane
        .publish_worker_artifact(&WorkerArtifactReport {
            claim: first_grant.claim(),
            observed_at: 110,
            artifact: artifact.clone(),
        })
        .await
        .expect("current worker artifact");
    assert_eq!(accepted.publish, artifact);

    lease_port
        .release(&TaskLeaseCloseRequest {
            claim: first_grant.claim(),
            observed_at: 120,
            reason: "worker_done".to_string(),
        })
        .await
        .expect("release first lease");
    let second_grant = lease_port
        .dispatch(&TaskLeaseDispatchRequest {
            execution_task_id: fixture.task_id.clone(),
            worker_incarnation_id: fixture.incarnation_id.clone(),
            observed_at: 121,
            expires_at: 220,
        })
        .await
        .expect("second lease");
    assert!(second_grant.fencing_token > first_grant.fencing_token);

    let stale = control_plane
        .publish_worker_artifact(&WorkerArtifactReport {
            claim: first_grant.claim(),
            observed_at: 130,
            artifact: accepted.publish.clone(),
        })
        .await
        .expect_err("old token artifact retry");
    assert_eq!(
        stale.lease_rejection_reason(),
        Some(TaskLeaseRejectionReason::StaleFencingToken)
    );
    let artifact_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_artifacts")
        .fetch_one(fixture.store.pool())
        .await
        .expect("artifact count");
    assert_eq!(artifact_count, 1);

    let rejection: (String, String) = sqlx::query_as(
        "SELECT event_type, payload_json FROM execution_audit_events \
         WHERE event_type='execution.report_rejected'",
    )
    .fetch_one(fixture.store.pool())
    .await
    .expect("rejection audit");
    assert_eq!(rejection.0, "execution.report_rejected");
    let payload: serde_json::Value = serde_json::from_str(&rejection.1).expect("payload JSON");
    assert_eq!(payload["operation"], "artifact.publish");
    assert_eq!(payload["lease_id"], first_grant.lease_id.as_str());
    assert_eq!(payload["fencing_token"], first_grant.fencing_token.value());
    assert_eq!(payload["reason"], "stale_fencing_token");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn worker_usage_report_rejects_superseded_or_terminal_lease_and_audits() {
    let fixture = fixture().await;
    let lease_port = SqliteTaskLeaseControlPlane::new(fixture.store.pool().clone());
    let control_plane =
        SqliteExecutionEvidenceControlPlane::new(fixture.store.pool().clone(), lease_port.clone());
    let old_grant = lease_port
        .dispatch(&TaskLeaseDispatchRequest {
            execution_task_id: fixture.task_id.clone(),
            worker_incarnation_id: fixture.incarnation_id.clone(),
            observed_at: 100,
            expires_at: 200,
        })
        .await
        .expect("old lease");
    let old_usage = usage(
        "ae_01ARZ3NDEKTSV4RRFFQ69G5FB9",
        "usage-stale-worker",
        UsageMetric::OutputTokens,
        5,
        &fixture,
    );

    let next_incarnation_id = WorkerIncarnationId::new();
    worker_repo::register_incarnation(
        fixture.store.pool(),
        &fixture.worker_id,
        WorkerRegistration {
            id: next_incarnation_id.clone(),
            daemon_version: "0.0.0-p271-restart".to_string(),
            host_name: "host-b".to_string(),
            network_zone: Some("dev".to_string()),
            capabilities: json!({"runtime": ["codex"]}),
        },
    )
    .await
    .expect("worker restart");
    let stale_worker = control_plane
        .record_worker_usage(&WorkerUsageReport {
            claim: old_grant.claim(),
            observed_at: 110,
            measurement: old_usage,
        })
        .await
        .expect_err("superseded worker usage");
    assert_eq!(
        stale_worker.lease_rejection_reason(),
        Some(TaskLeaseRejectionReason::StaleWorkerIncarnation)
    );

    let current_grant = lease_port
        .dispatch(&TaskLeaseDispatchRequest {
            execution_task_id: fixture.task_id.clone(),
            worker_incarnation_id: next_incarnation_id.clone(),
            observed_at: 120,
            expires_at: 220,
        })
        .await
        .expect("current lease");
    lease_port
        .release(&TaskLeaseCloseRequest {
            claim: current_grant.claim(),
            observed_at: 130,
            reason: "worker_done".to_string(),
        })
        .await
        .expect("terminal lease");
    let mut terminal_links = fixture.links.clone();
    terminal_links.worker_incarnation_id = Some(next_incarnation_id.clone());
    terminal_links.runtime_session_id = None;
    terminal_links.runtime_attempt_id = None;
    let terminal_usage = UsageMeasurement {
        id: AuditEventId::from_string("ae_01ARZ3NDEKTSV4RRFFQ69G5FBA"),
        idempotency_scope: format!("run:{}", fixture.run_id),
        idempotency_key: "usage-terminal".to_string(),
        actor_kind: AuditActorKind::Worker,
        actor_ref: next_incarnation_id.to_string(),
        metric: UsageMetric::OutputTokens,
        quantity: 7,
        provider: Some("openai".to_string()),
        model: Some("gpt-5".to_string()),
        links: terminal_links,
        measured_at: 131,
    };
    let terminal = control_plane
        .record_worker_usage(&WorkerUsageReport {
            claim: current_grant.claim(),
            observed_at: 131,
            measurement: terminal_usage,
        })
        .await
        .expect_err("terminal lease usage");
    assert_eq!(
        terminal.lease_rejection_reason(),
        Some(TaskLeaseRejectionReason::TerminalLease)
    );

    let usage_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_audit_events WHERE event_type='usage.measured'",
    )
    .fetch_one(fixture.store.pool())
    .await
    .expect("usage count");
    assert_eq!(usage_count, 0);
    let rejection_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_audit_events \
         WHERE event_type='execution.report_rejected'",
    )
    .fetch_one(fixture.store.pool())
    .await
    .expect("rejection count");
    assert_eq!(rejection_count, 2);
}
