//! `SQLite` adapter for execution artifact, audit, usage, and certification APIs.

use std::collections::BTreeMap;

use agentd_core::ports::{
    ArtifactCursor, ArtifactIndexPort, ArtifactListRequest, ArtifactPage, AuditActorKind,
    AuditPage, AuditReadRequest, CertificationReferenceAppend, CertificationReferenceKind,
    CertificationReferencePort, CertificationReferenceRecord, ExecutionArtifactKind,
    ExecutionArtifactPublish, ExecutionArtifactRecord, ExecutionAuditAppend, ExecutionAuditPort,
    ExecutionAuditRecord, ExecutionEvidenceError, ExecutionEvidenceLinks, ExecutionSnapshotLink,
    TaskLeaseError, TaskLeasePort, TaskLeaseRejectionReason, UsageLedgerPort, UsageMeasurement,
    UsageMetric, UsagePage, UsageReadRequest, UsageRecord, UsageTotal, UsageTotals,
    WorkerArtifactReport, WorkerUsageReport,
};
use agentd_core::types::{AuditEventId, ExecutionArtifactId, RunId, TaskLeaseClaim};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::StoreError;
use crate::execution_artifact_repo::{
    self, CertificationRefKind as StoreCertificationRefKind,
    ExecutionArtifactCreate as StoreArtifactCreate, ExecutionArtifactKind as StoreArtifactKind,
    ExecutionArtifactRecord as StoreArtifactRecord,
};
use crate::execution_audit_repo::{
    self, AuditActorKind as StoreAuditActorKind, AuditEventCreate as StoreAuditCreate,
    AuditEventRecord as StoreAuditRecord,
};
use crate::runtime_session_repo::ExecutionSnapshotRef;

#[derive(Debug, Clone)]
pub struct SqliteExecutionEvidenceControlPlane<L> {
    pool: SqlitePool,
    lease_port: L,
}

impl<L> SqliteExecutionEvidenceControlPlane<L> {
    #[must_use]
    pub fn new(pool: SqlitePool, lease_port: L) -> Self {
        Self { pool, lease_port }
    }
}

impl<L> SqliteExecutionEvidenceControlPlane<L>
where
    L: TaskLeasePort,
{
    async fn authorize_worker_evidence(
        &self,
        operation: &str,
        claim: &TaskLeaseClaim,
        observed_at: i64,
        links: &ExecutionEvidenceLinks,
    ) -> Result<(), ExecutionEvidenceError> {
        validate_worker_links(claim, links)?;
        match self.lease_port.validate_claim(claim, observed_at).await {
            Ok(_) => Ok(()),
            Err(TaskLeaseError::Rejected { reason, message }) => {
                self.append_rejection_audit(operation, claim, observed_at, links, reason)
                    .await?;
                Err(ExecutionEvidenceError::LeaseRejected { reason, message })
            }
            Err(TaskLeaseError::Invalid(message)) => Err(ExecutionEvidenceError::Invalid(message)),
            Err(TaskLeaseError::NotFound(message) | TaskLeaseError::Conflict(message)) => {
                let reason = TaskLeaseRejectionReason::NotCurrentLease;
                self.append_rejection_audit(operation, claim, observed_at, links, reason)
                    .await?;
                Err(ExecutionEvidenceError::LeaseRejected { reason, message })
            }
            Err(TaskLeaseError::Unavailable(message)) => {
                Err(ExecutionEvidenceError::Unavailable(message))
            }
        }
    }

    async fn authorize_artifact_upload(
        &self,
        request: &WorkerArtifactReport,
    ) -> Result<(), ExecutionEvidenceError> {
        let fencing_token = i64::try_from(request.claim.fencing_token.value()).map_err(|_| {
            ExecutionEvidenceError::Invalid("fencing token exceeds SQLite range".to_string())
        })?;
        let acknowledged: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM enterprise_artifact_upload_acknowledgements \
             WHERE upload_id = ? AND execution_artifact_id = ? AND execution_task_id = ? \
               AND worker_incarnation_id = ? AND lease_id = ? AND fencing_token = ? \
               AND artifact_sha256 = ? AND acknowledged_at <= ?",
        )
        .bind(request.upload_id.as_str())
        .bind(request.artifact.id.as_str())
        .bind(request.claim.execution_task_id.as_str())
        .bind(request.claim.worker_incarnation_id.as_str())
        .bind(request.claim.lease_id.as_str())
        .bind(fencing_token)
        .bind(&request.artifact.content_sha256)
        .bind(request.observed_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| ExecutionEvidenceError::Unavailable(error.to_string()))?;
        if acknowledged == 1 {
            return Ok(());
        }
        let reason = TaskLeaseRejectionReason::NotCurrentLease;
        self.append_rejection_audit(
            "artifact.upload_ack",
            &request.claim,
            request.observed_at,
            &request.artifact.links,
            reason,
        )
        .await?;
        Err(ExecutionEvidenceError::LeaseRejected {
            reason,
            message: "artifact upload lacks a current fenced acknowledgement".to_string(),
        })
    }

    async fn append_rejection_audit(
        &self,
        operation: &str,
        claim: &TaskLeaseClaim,
        observed_at: i64,
        links: &ExecutionEvidenceLinks,
        reason: TaskLeaseRejectionReason,
    ) -> Result<(), ExecutionEvidenceError> {
        let payload = serde_json::json!({
            "operation": operation,
            "execution_task_id": claim.execution_task_id,
            "worker_incarnation_id": claim.worker_incarnation_id,
            "lease_id": claim.lease_id,
            "fencing_token": claim.fencing_token.value(),
            "reason": reason.as_str(),
        });
        let audit_id = AuditEventId::new();
        let request = ExecutionAuditAppend {
            id: audit_id.clone(),
            idempotency_scope: format!("lease-rejection:{}", claim.execution_task_id),
            idempotency_key: audit_id.to_string(),
            event_type: REJECTION_EVENT_TYPE.to_string(),
            actor_kind: AuditActorKind::Worker,
            actor_ref: claim.worker_incarnation_id.to_string(),
            payload_sha256: sha256_json(&payload)?,
            payload,
            links: links.clone(),
            execution_artifact_id: None,
            occurred_at: observed_at,
        };
        self.append_audit(&request)
            .await
            .map(|_| ())
            .map_err(|error| {
                ExecutionEvidenceError::Unavailable(format!(
                    "required rejection audit failed: {error}"
                ))
            })
    }
}

#[async_trait::async_trait]
impl<L> ArtifactIndexPort for SqliteExecutionEvidenceControlPlane<L>
where
    L: agentd_core::ports::TaskLeasePort,
{
    async fn publish_artifact(
        &self,
        request: &ExecutionArtifactPublish,
    ) -> Result<ExecutionArtifactRecord, ExecutionEvidenceError> {
        if let Some(existing) = execution_artifact_repo::get_artifact(&self.pool, &request.id)
            .await
            .map_err(map_store_error)?
        {
            let existing = artifact_to_api(existing);
            if existing.publish == *request {
                return Ok(existing);
            }
            return Err(ExecutionEvidenceError::Conflict(format!(
                "artifact {} was reused with a changed envelope",
                request.id
            )));
        }
        let record =
            execution_artifact_repo::create_artifact(&self.pool, artifact_to_store(request))
                .await
                .map_err(map_store_error)?;
        Ok(artifact_to_api(record))
    }

    async fn publish_worker_artifact(
        &self,
        request: &WorkerArtifactReport,
    ) -> Result<ExecutionArtifactRecord, ExecutionEvidenceError> {
        self.authorize_worker_evidence(
            "artifact.publish",
            &request.claim,
            request.observed_at,
            &request.artifact.links,
        )
        .await?;
        self.authorize_artifact_upload(request).await?;
        self.publish_artifact(&request.artifact).await
    }

    async fn get_artifact(
        &self,
        id: &ExecutionArtifactId,
    ) -> Result<Option<ExecutionArtifactRecord>, ExecutionEvidenceError> {
        let record = execution_artifact_repo::get_artifact(&self.pool, id)
            .await
            .map_err(map_store_error)?;
        Ok(record.map(artifact_to_api))
    }

    async fn list_artifacts(
        &self,
        request: &ArtifactListRequest,
    ) -> Result<ArtifactPage, ExecutionEvidenceError> {
        if request
            .cursor
            .as_ref()
            .is_some_and(|cursor| cursor.created_at < 0)
        {
            return Err(ExecutionEvidenceError::Invalid(
                "artifact cursor timestamp must be non-negative".to_string(),
            ));
        }
        let fetch_limit = request.limit.value().checked_add(1).ok_or_else(|| {
            ExecutionEvidenceError::Invalid("artifact page limit overflow".to_string())
        })?;
        let cursor = request
            .cursor
            .as_ref()
            .map(|cursor| (cursor.created_at, &cursor.id));
        let mut records = execution_artifact_repo::list_artifacts_by_run(
            &self.pool,
            &request.execution_run_id,
            cursor,
            fetch_limit,
        )
        .await
        .map_err(map_store_error)?
        .into_iter()
        .map(artifact_to_api)
        .collect::<Vec<_>>();
        let has_more = records.len() > usize::from(request.limit.value());
        if has_more {
            records.pop();
        }
        let next_cursor = if has_more {
            records.last().map(|last| ArtifactCursor {
                created_at: last.created_at,
                id: last.publish.id.clone(),
            })
        } else {
            None
        };
        Ok(ArtifactPage {
            records,
            next_cursor,
        })
    }
}

#[async_trait::async_trait]
impl<L> ExecutionAuditPort for SqliteExecutionEvidenceControlPlane<L>
where
    L: agentd_core::ports::TaskLeasePort,
{
    async fn append_audit(
        &self,
        request: &ExecutionAuditAppend,
    ) -> Result<ExecutionAuditRecord, ExecutionEvidenceError> {
        execution_audit_repo::append_event(&self.pool, audit_to_store(request))
            .await
            .map_err(map_store_error)
            .and_then(audit_to_api)
    }

    async fn read_audit(
        &self,
        request: &AuditReadRequest,
    ) -> Result<AuditPage, ExecutionEvidenceError> {
        let after_sequence = i64::try_from(request.after_sequence).map_err(|_| {
            ExecutionEvidenceError::Invalid("audit cursor exceeds SQLite range".to_string())
        })?;
        let fetch_limit = request.limit.value().checked_add(1).ok_or_else(|| {
            ExecutionEvidenceError::Invalid("audit page limit overflow".to_string())
        })?;
        let mut records = execution_audit_repo::read_page(
            &self.pool,
            &request.execution_run_id,
            after_sequence,
            fetch_limit,
        )
        .await
        .map_err(map_store_error)?
        .into_iter()
        .map(audit_to_api)
        .collect::<Result<Vec<_>, _>>()?;
        let has_more = records.len() > usize::from(request.limit.value());
        if has_more {
            records.pop();
        }
        let next_after_sequence = if has_more {
            records.last().map(|record| record.sequence)
        } else {
            None
        };
        Ok(AuditPage {
            records,
            next_after_sequence,
        })
    }
}

#[async_trait::async_trait]
impl<L> UsageLedgerPort for SqliteExecutionEvidenceControlPlane<L>
where
    L: agentd_core::ports::TaskLeasePort,
{
    async fn record_usage(
        &self,
        request: &UsageMeasurement,
    ) -> Result<UsageRecord, ExecutionEvidenceError> {
        validate_usage(request)?;
        let payload = UsagePayload {
            metric: request.metric,
            quantity: request.quantity,
            provider: request.provider.clone(),
            model: request.model.clone(),
        };
        let payload_value = serde_json::to_value(&payload)
            .map_err(|error| ExecutionEvidenceError::Invalid(error.to_string()))?;
        let payload_sha256 = sha256_json(&payload_value)?;
        let audit = ExecutionAuditAppend {
            id: request.id.clone(),
            idempotency_scope: request.idempotency_scope.clone(),
            idempotency_key: request.idempotency_key.clone(),
            event_type: USAGE_EVENT_TYPE.to_string(),
            actor_kind: request.actor_kind,
            actor_ref: request.actor_ref.clone(),
            payload_sha256,
            payload: payload_value,
            links: request.links.clone(),
            execution_artifact_id: None,
            occurred_at: request.measured_at,
        };
        let record = self.append_audit(&audit).await?;
        usage_from_audit(record)
    }

    async fn record_worker_usage(
        &self,
        request: &WorkerUsageReport,
    ) -> Result<UsageRecord, ExecutionEvidenceError> {
        if request.measurement.actor_kind != AuditActorKind::Worker
            || request.measurement.actor_ref != request.claim.worker_incarnation_id.as_str()
        {
            return Err(ExecutionEvidenceError::Invalid(
                "worker usage actor does not match lease claim".to_string(),
            ));
        }
        self.authorize_worker_evidence(
            "usage.record",
            &request.claim,
            request.observed_at,
            &request.measurement.links,
        )
        .await?;
        self.record_usage(&request.measurement).await
    }

    async fn read_usage(
        &self,
        request: &UsageReadRequest,
    ) -> Result<UsagePage, ExecutionEvidenceError> {
        let after_sequence = sequence_to_i64(request.after_sequence, "usage cursor")?;
        let fetch_limit = request.limit.value().checked_add(1).ok_or_else(|| {
            ExecutionEvidenceError::Invalid("usage page limit overflow".to_string())
        })?;
        let mut records = execution_audit_repo::read_event_type_page(
            &self.pool,
            &request.execution_run_id,
            USAGE_EVENT_TYPE,
            after_sequence,
            fetch_limit,
        )
        .await
        .map_err(map_store_error)?
        .into_iter()
        .map(audit_to_api)
        .map(|record| record.and_then(usage_from_audit))
        .collect::<Result<Vec<_>, _>>()?;
        let has_more = records.len() > usize::from(request.limit.value());
        if has_more {
            records.pop();
        }
        let next_after_sequence = if has_more {
            records.last().map(|record| record.sequence)
        } else {
            None
        };
        Ok(UsagePage {
            records,
            next_after_sequence,
        })
    }

    async fn usage_totals(
        &self,
        execution_run_id: &RunId,
    ) -> Result<UsageTotals, ExecutionEvidenceError> {
        let mut totals = BTreeMap::<UsageMetric, u64>::new();
        let mut after_sequence = 0_i64;
        loop {
            let records = execution_audit_repo::read_event_type_page(
                &self.pool,
                execution_run_id,
                USAGE_EVENT_TYPE,
                after_sequence,
                200,
            )
            .await
            .map_err(map_store_error)?;
            if records.is_empty() {
                break;
            }
            for record in records.iter().cloned() {
                let usage = usage_from_audit(audit_to_api(record)?)?;
                let total = totals.entry(usage.measurement.metric).or_default();
                *total = total
                    .checked_add(usage.measurement.quantity)
                    .ok_or_else(|| {
                        ExecutionEvidenceError::Conflict(format!(
                            "usage total overflow for {}",
                            usage.measurement.metric
                        ))
                    })?;
            }
            let Some(last) = records.last() else {
                break;
            };
            after_sequence = last.sequence;
            if records.len() < 200 {
                break;
            }
        }
        Ok(UsageTotals {
            execution_run_id: execution_run_id.clone(),
            totals: totals
                .into_iter()
                .map(|(metric, quantity)| UsageTotal { metric, quantity })
                .collect(),
        })
    }
}

const USAGE_EVENT_TYPE: &str = "usage.measured";
const REJECTION_EVENT_TYPE: &str = "execution.report_rejected";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct UsagePayload {
    metric: UsageMetric,
    quantity: u64,
    provider: Option<String>,
    model: Option<String>,
}

fn usage_from_audit(record: ExecutionAuditRecord) -> Result<UsageRecord, ExecutionEvidenceError> {
    if record.append.event_type != USAGE_EVENT_TYPE {
        return Err(ExecutionEvidenceError::Invalid(format!(
            "audit event {} is not usage",
            record.append.id
        )));
    }
    let payload: UsagePayload =
        serde_json::from_value(record.append.payload.clone()).map_err(|error| {
            ExecutionEvidenceError::Invalid(format!("invalid usage payload: {error}"))
        })?;
    Ok(UsageRecord {
        measurement: UsageMeasurement {
            id: record.append.id,
            idempotency_scope: record.append.idempotency_scope,
            idempotency_key: record.append.idempotency_key,
            actor_kind: record.append.actor_kind,
            actor_ref: record.append.actor_ref,
            metric: payload.metric,
            quantity: payload.quantity,
            provider: payload.provider,
            model: payload.model,
            links: record.append.links,
            measured_at: record.append.occurred_at,
        },
        sequence: record.sequence,
        recorded_at: record.recorded_at,
    })
}

fn validate_usage(request: &UsageMeasurement) -> Result<(), ExecutionEvidenceError> {
    if request.measured_at < 0 {
        return Err(ExecutionEvidenceError::Invalid(
            "usage measured_at must be non-negative".to_string(),
        ));
    }
    for (value, field) in [
        (request.provider.as_deref(), "usage provider"),
        (request.model.as_deref(), "usage model"),
    ] {
        if value.is_some_and(|value| value.trim().is_empty()) {
            return Err(ExecutionEvidenceError::Invalid(format!(
                "{field} must not be empty"
            )));
        }
    }
    Ok(())
}

fn validate_worker_links(
    claim: &TaskLeaseClaim,
    links: &ExecutionEvidenceLinks,
) -> Result<(), ExecutionEvidenceError> {
    if links.execution_task_id.as_ref() != Some(&claim.execution_task_id)
        || links.worker_incarnation_id.as_ref() != Some(&claim.worker_incarnation_id)
    {
        return Err(ExecutionEvidenceError::Invalid(
            "worker evidence links do not match lease claim".to_string(),
        ));
    }
    Ok(())
}

fn sha256_json(value: &serde_json::Value) -> Result<String, ExecutionEvidenceError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| ExecutionEvidenceError::Invalid(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn sequence_to_i64(sequence: u64, field: &str) -> Result<i64, ExecutionEvidenceError> {
    i64::try_from(sequence).map_err(|_| {
        ExecutionEvidenceError::Invalid(format!("{field} exceeds SQLite INTEGER range"))
    })
}

#[async_trait::async_trait]
impl<L> CertificationReferencePort for SqliteExecutionEvidenceControlPlane<L>
where
    L: agentd_core::ports::TaskLeasePort,
{
    async fn append_certification_reference(
        &self,
        request: &CertificationReferenceAppend,
    ) -> Result<CertificationReferenceRecord, ExecutionEvidenceError> {
        execution_artifact_repo::append_certification_ref(
            &self.pool,
            &request.execution_artifact_id,
            &request.authority_key,
            cert_kind_to_store(request.kind),
            &request.external_ref,
        )
        .await
        .map_err(map_store_error)
        .and_then(cert_to_api)
    }

    async fn list_certification_references(
        &self,
        artifact_id: &ExecutionArtifactId,
    ) -> Result<Vec<CertificationReferenceRecord>, ExecutionEvidenceError> {
        execution_artifact_repo::list_certification_refs(&self.pool, artifact_id)
            .await
            .map_err(map_store_error)?
            .into_iter()
            .map(cert_to_api)
            .collect()
    }
}

fn artifact_to_store(request: &ExecutionArtifactPublish) -> StoreArtifactCreate {
    StoreArtifactCreate {
        id: request.id.clone(),
        kind: artifact_kind_to_store(request.kind),
        content_sha256: request.content_sha256.clone(),
        size_bytes: request.size_bytes,
        media_type: request.media_type.clone(),
        storage_ref: request.storage_ref.clone(),
        provenance: request.provenance.clone(),
        execution_run_id: request.links.execution_run_id.clone(),
        execution_task_id: request.links.execution_task_id.clone(),
        runtime_session_id: request.links.runtime_session_id.clone(),
        runtime_attempt_id: request.links.runtime_attempt_id.clone(),
        snapshot: snapshot_to_store(&request.links.snapshot),
        target_repository_id: request.links.target_repository_id.clone(),
        target_base_commit: request.links.target_base_commit.clone(),
        producer_worker_incarnation_id: request.links.worker_incarnation_id.clone(),
    }
}

fn artifact_to_api(record: StoreArtifactRecord) -> ExecutionArtifactRecord {
    ExecutionArtifactRecord {
        publish: ExecutionArtifactPublish {
            id: record.id,
            kind: artifact_kind_to_api(record.kind),
            content_sha256: record.content_sha256,
            size_bytes: record.size_bytes,
            media_type: record.media_type,
            storage_ref: record.storage_ref,
            provenance: record.provenance,
            links: ExecutionEvidenceLinks {
                execution_run_id: record.execution_run_id,
                execution_task_id: record.execution_task_id,
                runtime_session_id: record.runtime_session_id,
                runtime_attempt_id: record.runtime_attempt_id,
                worker_incarnation_id: record.producer_worker_incarnation_id,
                snapshot: snapshot_to_api(record.snapshot),
                target_repository_id: record.target_repository_id,
                target_base_commit: record.target_base_commit,
            },
        },
        created_at: record.created_at,
    }
}

fn audit_to_store(request: &ExecutionAuditAppend) -> StoreAuditCreate {
    StoreAuditCreate {
        id: request.id.clone(),
        idempotency_scope: request.idempotency_scope.clone(),
        idempotency_key: request.idempotency_key.clone(),
        event_type: request.event_type.clone(),
        actor_kind: audit_actor_to_store(request.actor_kind),
        actor_ref: request.actor_ref.clone(),
        payload_sha256: request.payload_sha256.clone(),
        payload: request.payload.clone(),
        execution_run_id: request.links.execution_run_id.clone(),
        execution_task_id: request.links.execution_task_id.clone(),
        runtime_session_id: request.links.runtime_session_id.clone(),
        runtime_attempt_id: request.links.runtime_attempt_id.clone(),
        execution_artifact_id: request.execution_artifact_id.clone(),
        worker_incarnation_id: request.links.worker_incarnation_id.clone(),
        snapshot: snapshot_to_store(&request.links.snapshot),
        target_repository_id: request.links.target_repository_id.clone(),
        target_base_commit: request.links.target_base_commit.clone(),
        occurred_at: request.occurred_at,
    }
}

fn audit_to_api(record: StoreAuditRecord) -> Result<ExecutionAuditRecord, ExecutionEvidenceError> {
    Ok(ExecutionAuditRecord {
        append: ExecutionAuditAppend {
            id: record.id,
            idempotency_scope: record.idempotency_scope,
            idempotency_key: record.idempotency_key,
            event_type: record.event_type,
            actor_kind: audit_actor_to_api(record.actor_kind),
            actor_ref: record.actor_ref,
            payload_sha256: record.payload_sha256,
            payload: record.payload,
            links: ExecutionEvidenceLinks {
                execution_run_id: record.execution_run_id,
                execution_task_id: record.execution_task_id,
                runtime_session_id: record.runtime_session_id,
                runtime_attempt_id: record.runtime_attempt_id,
                worker_incarnation_id: record.worker_incarnation_id,
                snapshot: snapshot_to_api(record.snapshot),
                target_repository_id: record.target_repository_id,
                target_base_commit: record.target_base_commit,
            },
            execution_artifact_id: record.execution_artifact_id,
            occurred_at: record.occurred_at,
        },
        sequence: u64::try_from(record.sequence).map_err(|_| {
            ExecutionEvidenceError::Unavailable("negative audit sequence".to_string())
        })?,
        recorded_at: record.recorded_at,
    })
}

fn cert_to_api(
    record: execution_artifact_repo::CertificationRefRecord,
) -> Result<CertificationReferenceRecord, ExecutionEvidenceError> {
    Ok(CertificationReferenceRecord {
        id: u64::try_from(record.id).map_err(|_| {
            ExecutionEvidenceError::Unavailable("negative certification reference id".to_string())
        })?,
        append: CertificationReferenceAppend {
            execution_artifact_id: record.execution_artifact_id,
            authority_key: record.authority_key,
            kind: cert_kind_to_api(record.kind),
            external_ref: record.external_ref,
        },
        recorded_at: record.recorded_at,
    })
}

fn snapshot_to_store(snapshot: &ExecutionSnapshotLink) -> ExecutionSnapshotRef {
    ExecutionSnapshotRef {
        authority_key: snapshot.authority_key.clone(),
        resource_kind: snapshot.resource_kind.clone(),
        resource_id: snapshot.resource_id.clone(),
        resource_version: snapshot.resource_version.clone(),
        content_sha256: snapshot.content_sha256.clone(),
    }
}

fn snapshot_to_api(snapshot: ExecutionSnapshotRef) -> ExecutionSnapshotLink {
    ExecutionSnapshotLink {
        authority_key: snapshot.authority_key,
        resource_kind: snapshot.resource_kind,
        resource_id: snapshot.resource_id,
        resource_version: snapshot.resource_version,
        content_sha256: snapshot.content_sha256,
    }
}

fn artifact_kind_to_store(kind: ExecutionArtifactKind) -> StoreArtifactKind {
    match kind {
        ExecutionArtifactKind::Requirements => StoreArtifactKind::Requirements,
        ExecutionArtifactKind::Spec => StoreArtifactKind::Spec,
        ExecutionArtifactKind::Plan => StoreArtifactKind::Plan,
        ExecutionArtifactKind::Review => StoreArtifactKind::Review,
        ExecutionArtifactKind::RuntimeSummary => StoreArtifactKind::RuntimeSummary,
        ExecutionArtifactKind::Transcript => StoreArtifactKind::Transcript,
        ExecutionArtifactKind::Log => StoreArtifactKind::Log,
        ExecutionArtifactKind::Patch => StoreArtifactKind::Patch,
        ExecutionArtifactKind::Commit => StoreArtifactKind::Commit,
        ExecutionArtifactKind::TestReport => StoreArtifactKind::TestReport,
    }
}

fn artifact_kind_to_api(kind: StoreArtifactKind) -> ExecutionArtifactKind {
    match kind {
        StoreArtifactKind::Requirements => ExecutionArtifactKind::Requirements,
        StoreArtifactKind::Spec => ExecutionArtifactKind::Spec,
        StoreArtifactKind::Plan => ExecutionArtifactKind::Plan,
        StoreArtifactKind::Review => ExecutionArtifactKind::Review,
        StoreArtifactKind::RuntimeSummary => ExecutionArtifactKind::RuntimeSummary,
        StoreArtifactKind::Transcript => ExecutionArtifactKind::Transcript,
        StoreArtifactKind::Log => ExecutionArtifactKind::Log,
        StoreArtifactKind::Patch => ExecutionArtifactKind::Patch,
        StoreArtifactKind::Commit => ExecutionArtifactKind::Commit,
        StoreArtifactKind::TestReport => ExecutionArtifactKind::TestReport,
    }
}

fn audit_actor_to_store(kind: AuditActorKind) -> StoreAuditActorKind {
    match kind {
        AuditActorKind::ControlPlane => StoreAuditActorKind::ControlPlane,
        AuditActorKind::Worker => StoreAuditActorKind::Worker,
        AuditActorKind::AgentProfile => StoreAuditActorKind::AgentProfile,
        AuditActorKind::Operator => StoreAuditActorKind::Operator,
        AuditActorKind::ProjectAuthority => StoreAuditActorKind::ProjectAuthority,
        AuditActorKind::CertificationAuthority => StoreAuditActorKind::CertificationAuthority,
        AuditActorKind::System => StoreAuditActorKind::System,
        AuditActorKind::Import => StoreAuditActorKind::Import,
    }
}

fn audit_actor_to_api(kind: StoreAuditActorKind) -> AuditActorKind {
    match kind {
        StoreAuditActorKind::ControlPlane => AuditActorKind::ControlPlane,
        StoreAuditActorKind::Worker => AuditActorKind::Worker,
        StoreAuditActorKind::AgentProfile => AuditActorKind::AgentProfile,
        StoreAuditActorKind::Operator => AuditActorKind::Operator,
        StoreAuditActorKind::ProjectAuthority => AuditActorKind::ProjectAuthority,
        StoreAuditActorKind::CertificationAuthority => AuditActorKind::CertificationAuthority,
        StoreAuditActorKind::System => AuditActorKind::System,
        StoreAuditActorKind::Import => AuditActorKind::Import,
    }
}

fn cert_kind_to_store(kind: CertificationReferenceKind) -> StoreCertificationRefKind {
    match kind {
        CertificationReferenceKind::Request => StoreCertificationRefKind::Request,
        CertificationReferenceKind::Result => StoreCertificationRefKind::Result,
        CertificationReferenceKind::Signature => StoreCertificationRefKind::Signature,
        CertificationReferenceKind::Attestation => StoreCertificationRefKind::Attestation,
    }
}

fn cert_kind_to_api(kind: StoreCertificationRefKind) -> CertificationReferenceKind {
    match kind {
        StoreCertificationRefKind::Request => CertificationReferenceKind::Request,
        StoreCertificationRefKind::Result => CertificationReferenceKind::Result,
        StoreCertificationRefKind::Signature => CertificationReferenceKind::Signature,
        StoreCertificationRefKind::Attestation => CertificationReferenceKind::Attestation,
    }
}

fn map_store_error(error: StoreError) -> ExecutionEvidenceError {
    match error {
        StoreError::NotFound => ExecutionEvidenceError::NotFound("record".to_string()),
        StoreError::Conflict(message) => ExecutionEvidenceError::Conflict(message),
        StoreError::Invariant(message) => ExecutionEvidenceError::Invalid(message),
        other => ExecutionEvidenceError::Unavailable(other.to_string()),
    }
}
