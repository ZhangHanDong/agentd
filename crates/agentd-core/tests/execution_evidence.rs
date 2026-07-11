use std::sync::Mutex;

use agentd_core::ports::{
    ArtifactCursor, ArtifactIndexPort, ArtifactListRequest, ArtifactPage, AuditActorKind,
    AuditPage, AuditReadRequest, CertificationReferenceAppend, CertificationReferenceKind,
    CertificationReferencePort, CertificationReferenceRecord, ExecutionArtifactKind,
    ExecutionArtifactPublish, ExecutionArtifactRecord, ExecutionAuditAppend, ExecutionAuditPort,
    ExecutionAuditRecord, ExecutionEvidenceError, ExecutionEvidenceLinks, ExecutionSnapshotLink,
    PageLimit, UsageLedgerPort, UsageMeasurement, UsageMetric, UsagePage, UsageReadRequest,
    UsageRecord, UsageTotal, UsageTotals, WorkerArtifactReport, WorkerUsageReport,
};
use agentd_core::types::{
    AuditEventId, ExecutionArtifactId, FencingToken, LeaseId, RunId, TaskLeaseClaim, TaskRunId,
    WorkerIncarnationId,
};
use serde_json::json;

fn links() -> ExecutionEvidenceLinks {
    ExecutionEvidenceLinks {
        execution_run_id: RunId::from_string("r_01ARZ3NDEKTSV4RRFFQ69G5FAV"),
        execution_task_id: Some(TaskRunId::from_string("tr_01ARZ3NDEKTSV4RRFFQ69G5FAW")),
        runtime_session_id: None,
        runtime_attempt_id: None,
        worker_incarnation_id: Some(WorkerIncarnationId::from_string(
            "wi_01ARZ3NDEKTSV4RRFFQ69G5FAX",
        )),
        snapshot: ExecutionSnapshotLink {
            authority_key: "specify:corp".to_string(),
            resource_kind: "execution_snapshot".to_string(),
            resource_id: "snapshot-1".to_string(),
            resource_version: "7".to_string(),
            content_sha256: "a".repeat(64),
        },
        target_repository_id: "repo-1".to_string(),
        target_base_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
    }
}

fn artifact_publish() -> ExecutionArtifactPublish {
    ExecutionArtifactPublish {
        id: ExecutionArtifactId::from_string("ar_01ARZ3NDEKTSV4RRFFQ69G5FAY"),
        kind: ExecutionArtifactKind::TestReport,
        content_sha256: "b".repeat(64),
        size_bytes: 42,
        media_type: "application/json".to_string(),
        storage_ref: "cas://test-report".to_string(),
        provenance: json!({"tool": "cargo-test"}),
        links: links(),
    }
}

fn artifact_record() -> ExecutionArtifactRecord {
    ExecutionArtifactRecord {
        publish: artifact_publish(),
        created_at: 100,
    }
}

fn audit_append() -> ExecutionAuditAppend {
    ExecutionAuditAppend {
        id: AuditEventId::from_string("ae_01ARZ3NDEKTSV4RRFFQ69G5FAZ"),
        idempotency_scope: "run:test".to_string(),
        idempotency_key: "event-1".to_string(),
        event_type: "artifact.recorded".to_string(),
        actor_kind: AuditActorKind::ControlPlane,
        actor_ref: "agentd".to_string(),
        payload_sha256: "c".repeat(64),
        payload: json!({"artifact": "test"}),
        links: links(),
        execution_artifact_id: Some(artifact_publish().id),
        occurred_at: 90,
    }
}

fn audit_record() -> ExecutionAuditRecord {
    ExecutionAuditRecord {
        append: audit_append(),
        sequence: 1,
        recorded_at: 100,
    }
}

fn usage_measurement() -> UsageMeasurement {
    UsageMeasurement {
        id: AuditEventId::from_string("ae_01ARZ3NDEKTSV4RRFFQ69G5FB0"),
        idempotency_scope: "run:test".to_string(),
        idempotency_key: "usage-1".to_string(),
        actor_kind: AuditActorKind::Worker,
        actor_ref: "wi_01ARZ3NDEKTSV4RRFFQ69G5FAX".to_string(),
        metric: UsageMetric::InputTokens,
        quantity: 17,
        provider: Some("openai".to_string()),
        model: Some("gpt-5".to_string()),
        links: links(),
        measured_at: 95,
    }
}

fn usage_record() -> UsageRecord {
    UsageRecord {
        measurement: usage_measurement(),
        sequence: 2,
        recorded_at: 100,
    }
}

fn claim() -> TaskLeaseClaim {
    TaskLeaseClaim {
        execution_task_id: links().execution_task_id.expect("task"),
        worker_incarnation_id: links().worker_incarnation_id.expect("worker"),
        lease_id: LeaseId::from_string("ls_01ARZ3NDEKTSV4RRFFQ69G5FB1"),
        fencing_token: FencingToken::new(3).expect("token"),
    }
}

#[derive(Default)]
struct RecordingPorts {
    calls: Mutex<Vec<String>>,
}

impl RecordingPorts {
    fn record(&self, call: String) {
        self.calls.lock().expect("calls lock").push(call);
    }
}

#[async_trait::async_trait]
impl ArtifactIndexPort for RecordingPorts {
    async fn publish_artifact(
        &self,
        request: &ExecutionArtifactPublish,
    ) -> Result<ExecutionArtifactRecord, ExecutionEvidenceError> {
        self.record(format!("artifact.publish:{}", request.id));
        Ok(artifact_record())
    }

    async fn publish_worker_artifact(
        &self,
        request: &WorkerArtifactReport,
    ) -> Result<ExecutionArtifactRecord, ExecutionEvidenceError> {
        self.record(format!("artifact.worker:{}", request.claim.lease_id));
        Ok(artifact_record())
    }

    async fn get_artifact(
        &self,
        id: &ExecutionArtifactId,
    ) -> Result<Option<ExecutionArtifactRecord>, ExecutionEvidenceError> {
        self.record(format!("artifact.get:{id}"));
        Ok(Some(artifact_record()))
    }

    async fn list_artifacts(
        &self,
        request: &ArtifactListRequest,
    ) -> Result<ArtifactPage, ExecutionEvidenceError> {
        self.record(format!(
            "artifact.list:{}:{}",
            request.execution_run_id,
            request.limit.value()
        ));
        Ok(ArtifactPage {
            records: vec![artifact_record()],
            next_cursor: None,
        })
    }
}

#[async_trait::async_trait]
impl ExecutionAuditPort for RecordingPorts {
    async fn append_audit(
        &self,
        request: &ExecutionAuditAppend,
    ) -> Result<ExecutionAuditRecord, ExecutionEvidenceError> {
        self.record(format!("audit.append:{}", request.id));
        Ok(audit_record())
    }

    async fn read_audit(
        &self,
        request: &AuditReadRequest,
    ) -> Result<AuditPage, ExecutionEvidenceError> {
        self.record(format!(
            "audit.read:{}:{}:{}",
            request.execution_run_id,
            request.after_sequence,
            request.limit.value()
        ));
        Ok(AuditPage {
            records: vec![audit_record()],
            next_after_sequence: Some(1),
        })
    }
}

#[async_trait::async_trait]
impl UsageLedgerPort for RecordingPorts {
    async fn record_usage(
        &self,
        request: &UsageMeasurement,
    ) -> Result<UsageRecord, ExecutionEvidenceError> {
        self.record(format!("usage.record:{}", request.id));
        Ok(usage_record())
    }

    async fn record_worker_usage(
        &self,
        request: &WorkerUsageReport,
    ) -> Result<UsageRecord, ExecutionEvidenceError> {
        self.record(format!("usage.worker:{}", request.claim.lease_id));
        Ok(usage_record())
    }

    async fn read_usage(
        &self,
        request: &UsageReadRequest,
    ) -> Result<UsagePage, ExecutionEvidenceError> {
        self.record(format!(
            "usage.read:{}:{}",
            request.execution_run_id,
            request.limit.value()
        ));
        Ok(UsagePage {
            records: vec![usage_record()],
            next_after_sequence: Some(2),
        })
    }

    async fn usage_totals(
        &self,
        execution_run_id: &RunId,
    ) -> Result<UsageTotals, ExecutionEvidenceError> {
        self.record(format!("usage.totals:{execution_run_id}"));
        Ok(UsageTotals {
            execution_run_id: execution_run_id.clone(),
            totals: vec![UsageTotal {
                metric: UsageMetric::InputTokens,
                quantity: 17,
            }],
        })
    }
}

#[async_trait::async_trait]
impl CertificationReferencePort for RecordingPorts {
    async fn append_certification_reference(
        &self,
        request: &CertificationReferenceAppend,
    ) -> Result<CertificationReferenceRecord, ExecutionEvidenceError> {
        self.record(format!("cert.append:{}", request.external_ref));
        Ok(CertificationReferenceRecord {
            id: 1,
            append: request.clone(),
            recorded_at: 100,
        })
    }

    async fn list_certification_references(
        &self,
        artifact_id: &ExecutionArtifactId,
    ) -> Result<Vec<CertificationReferenceRecord>, ExecutionEvidenceError> {
        self.record(format!("cert.list:{artifact_id}"));
        Ok(Vec::new())
    }
}

#[test]
fn execution_evidence_contract_types_are_closed_and_page_limits_are_validated() {
    assert!(PageLimit::new(0).is_err());
    assert!(PageLimit::new(201).is_err());
    let one = PageLimit::new(1).expect("one-row page");
    assert_eq!(one.value(), 1);
    assert_eq!(PageLimit::new(200).expect("max page").value(), 200);
    assert!(serde_json::from_str::<PageLimit>("0").is_err());
    assert!(serde_json::from_str::<PageLimit>("201").is_err());
    assert_eq!(
        serde_json::from_str::<PageLimit>("200")
            .expect("deserialize max page")
            .value(),
        200
    );

    for (kind, value) in [
        (ExecutionArtifactKind::Requirements, "requirements"),
        (ExecutionArtifactKind::Spec, "spec"),
        (ExecutionArtifactKind::Plan, "plan"),
        (ExecutionArtifactKind::Review, "review"),
        (ExecutionArtifactKind::RuntimeSummary, "runtime_summary"),
        (ExecutionArtifactKind::Transcript, "transcript"),
        (ExecutionArtifactKind::Log, "log"),
        (ExecutionArtifactKind::Patch, "patch"),
        (ExecutionArtifactKind::Commit, "commit"),
        (ExecutionArtifactKind::TestReport, "test_report"),
    ] {
        assert_eq!(kind.as_str(), value);
        assert_eq!(ExecutionArtifactKind::try_from(value).expect("kind"), kind);
    }
    for (metric, value) in [
        (UsageMetric::InputTokens, "input_tokens"),
        (UsageMetric::CachedInputTokens, "cached_input_tokens"),
        (UsageMetric::OutputTokens, "output_tokens"),
        (UsageMetric::ReasoningTokens, "reasoning_tokens"),
        (UsageMetric::ToolCalls, "tool_calls"),
        (UsageMetric::RuntimeMilliseconds, "runtime_milliseconds"),
        (UsageMetric::ArtifactBytes, "artifact_bytes"),
    ] {
        assert_eq!(metric.as_str(), value);
        assert_eq!(UsageMetric::try_from(value).expect("metric"), metric);
    }
    for (kind, value) in [
        (CertificationReferenceKind::Request, "request"),
        (CertificationReferenceKind::Result, "result"),
        (CertificationReferenceKind::Signature, "signature"),
        (CertificationReferenceKind::Attestation, "attestation"),
    ] {
        assert_eq!(kind.as_str(), value);
        assert_eq!(
            CertificationReferenceKind::try_from(value).expect("cert kind"),
            kind
        );
    }
}

#[tokio::test]
async fn execution_evidence_contract_ports_preserve_typed_requests() {
    let one = PageLimit::new(1).expect("one-row page");
    let ports = RecordingPorts::default();
    let artifact = artifact_publish();
    let artifact_list = ArtifactListRequest {
        execution_run_id: links().execution_run_id,
        cursor: Some(ArtifactCursor {
            created_at: 99,
            id: ExecutionArtifactId::new(),
        }),
        limit: one,
    };
    let worker_artifact = WorkerArtifactReport {
        claim: claim(),
        observed_at: 100,
        artifact: artifact.clone(),
    };
    ports.publish_artifact(&artifact).await.expect("publish");
    ports
        .publish_worker_artifact(&worker_artifact)
        .await
        .expect("worker publish");
    ports.get_artifact(&artifact.id).await.expect("get");
    ports.list_artifacts(&artifact_list).await.expect("list");

    let audit = audit_append();
    ports.append_audit(&audit).await.expect("audit append");
    ports
        .read_audit(&AuditReadRequest {
            execution_run_id: links().execution_run_id,
            after_sequence: 0,
            limit: one,
        })
        .await
        .expect("audit read");

    let usage = usage_measurement();
    ports.record_usage(&usage).await.expect("record usage");
    ports
        .record_worker_usage(&WorkerUsageReport {
            claim: claim(),
            observed_at: 100,
            measurement: usage.clone(),
        })
        .await
        .expect("worker usage");
    ports
        .read_usage(&UsageReadRequest {
            execution_run_id: links().execution_run_id.clone(),
            after_sequence: 0,
            limit: one,
        })
        .await
        .expect("usage read");
    ports
        .usage_totals(&links().execution_run_id)
        .await
        .expect("totals");

    let cert = CertificationReferenceAppend {
        execution_artifact_id: artifact.id.clone(),
        authority_key: "openfab:prod".to_string(),
        kind: CertificationReferenceKind::Request,
        external_ref: "of-request-1".to_string(),
    };
    ports
        .append_certification_reference(&cert)
        .await
        .expect("cert append");
    ports
        .list_certification_references(&artifact.id)
        .await
        .expect("cert list");

    assert_eq!(ports.calls.lock().expect("calls lock").len(), 12);
}
