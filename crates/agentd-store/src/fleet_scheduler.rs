//! SQLite implementation of the durable enterprise scheduler and worker fleet.

use std::collections::BTreeSet;
use std::sync::Arc;

use agentd_core::ports::{
    ArtifactUploadAck, ArtifactUploadAckRequest, FleetAssignment, FleetCancelRequest,
    FleetCompletionReport, FleetDenialReason, FleetExplain, FleetFailureReport,
    FleetHeartbeatRequest, FleetOutboxEvent, FleetPullRequest, FleetQueueStatus, FleetReapRequest,
    FleetReapSummary, FleetRenewRequest, FleetSchedulerError, FleetSchedulerPort,
    FleetSideEffectAdmission, FleetSideEffectRequest, FleetSubmitRequest, FleetTaskRecord,
    FleetTaskRequirements, PolicyRevocationPort, TaskLeaseDispatchRequest, WorkerAvailability,
};
use agentd_core::types::{
    ArtifactUploadId, AuthenticatedWorkload, AuthorityKey, ExecutionArtifactId, FencingToken,
    FleetOutboxId, LeaseId, LeaseStatus, OrganizationRef, PlacementCandidate, PlacementPolicy,
    ProjectExecutionSnapshotRef, ProjectRef, ProtectedAction, SecurityCheckpoint,
    SecurityEpochRequest, TaskLeaseClaim, TaskLeaseGrant, TaskRunId, WorkerId, WorkerIncarnationId,
    WorkerStatus, WorkloadRole,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::task_lease_control_plane::{
    ClaimAuthorization, Decision, authorize_claim, close_in_transaction, dispatch_in_transaction,
    renew_in_transaction, terminalize_active,
};
use crate::util::SqliteImmediateTransaction;

const MAX_FLEET_SET_VALUES: usize = 128;
const MAX_WORKER_SLOTS: u32 = 10_000;

const TASK_SELECT: &str = "SELECT queue.execution_task_id, queue.idempotency_key, queue.status, \
     queue.snapshot_authority_key, queue.snapshot_resource_id, queue.snapshot_resource_version, \
     queue.snapshot_content_sha256, queue.policy_revocation_epoch, queue.resource_class, \
     queue.required_capabilities_json, queue.quota_max_active, queue.priority, \
     queue.max_attempts, queue.attempt_count, queue.assigned_lease_id, \
     queue.assigned_worker_incarnation_id, queue.next_eligible_at, queue.outcome_sha256, \
     queue.block_code, queue.created_at, queue.updated_at, lease.fencing_token \
     FROM enterprise_fleet_queue AS queue \
     LEFT JOIN execution_task_leases AS lease ON lease.id = queue.assigned_lease_id";

#[derive(Clone)]
pub struct SqliteFleetScheduler {
    pool: SqlitePool,
    revocation: Arc<dyn PolicyRevocationPort>,
}

impl std::fmt::Debug for SqliteFleetScheduler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteFleetScheduler")
            .field("pool", &"[SQLITE]")
            .field("revocation", &"[CONFIGURED]")
            .finish()
    }
}

impl SqliteFleetScheduler {
    #[must_use]
    pub fn new(pool: SqlitePool, revocation: Arc<dyn PolicyRevocationPort>) -> Self {
        Self { pool, revocation }
    }

    #[must_use]
    pub const fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    async fn check_epoch(&self, request: &SecurityEpochRequest) -> Result<(), FleetSchedulerError> {
        let status = self
            .revocation
            .check_security_epoch(request)
            .await
            .map_err(|error| FleetSchedulerError::Unavailable(error.to_string()))?;
        if status.observed_at > request.observed_at
            || request.observed_at.saturating_sub(status.observed_at) > 60
        {
            return Err(FleetSchedulerError::Unavailable(
                "revocation authority returned stale or future state".to_string(),
            ));
        }
        status
            .validate_request(request)
            .map_err(|_| FleetSchedulerError::Denied(FleetDenialReason::RevocationEpochStale))?;
        status
            .validate_pinned_epoch(request.pinned_epoch)
            .map_err(|_| FleetSchedulerError::Denied(FleetDenialReason::RevocationEpochStale))
    }
}

#[async_trait::async_trait]
impl FleetSchedulerPort for SqliteFleetScheduler {
    async fn submit_task(
        &self,
        request: &FleetSubmitRequest,
    ) -> Result<FleetTaskRecord, FleetSchedulerError> {
        validate_submit(request)?;
        let submission_sha256 = sha256(request)?;
        let mut connection = begin_immediate(&self.pool).await?;
        if let Some(existing) = sqlx::query(
            "SELECT execution_task_id, submission_sha256 FROM enterprise_fleet_queue \
             WHERE execution_task_id = ? OR idempotency_key = ?",
        )
        .bind(request.execution_task_id.as_str())
        .bind(request.idempotency_key.trim())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?
        {
            let matches = existing.get::<String, _>("execution_task_id")
                == request.execution_task_id.as_str()
                && existing.get::<String, _>("submission_sha256") == submission_sha256;
            commit(&mut connection).await?;
            if !matches {
                return Err(FleetSchedulerError::Denied(
                    FleetDenialReason::DuplicateMismatch,
                ));
            }
            return get_task(&self.pool, &request.execution_task_id)
                .await?
                .ok_or_else(|| {
                    FleetSchedulerError::Unavailable(
                        "idempotent queue record disappeared".to_string(),
                    )
                });
        }

        let snapshot = &request.snapshot;
        let placement_policy_json = serde_json::to_string(&snapshot.placement_policy)
            .map_err(|error| FleetSchedulerError::Invalid(error.to_string()))?;
        let capabilities_json = serde_json::to_string(&request.requirements.required_capabilities)
            .map_err(|error| FleetSchedulerError::Invalid(error.to_string()))?;
        sqlx::query(
            "INSERT INTO enterprise_fleet_queue (\
                execution_task_id, idempotency_key, submission_sha256, status, \
                snapshot_authority_key, snapshot_resource_id, snapshot_resource_version, \
                snapshot_content_sha256, snapshot_valid_until, organization_authority_key, \
                organization_resource_id, organization_resource_version, project_authority_key, \
                project_resource_id, project_resource_version, rbac_policy_resource_id, \
                rbac_policy_resource_version, quota_policy_resource_id, \
                quota_policy_resource_version, policy_revocation_epoch, placement_policy_json, \
                resource_class, required_capabilities_json, quota_max_active, priority, \
                max_attempts, attempt_count, created_at, updated_at\
             ) VALUES (?, ?, ?, 'queued', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?)",
        )
        .bind(request.execution_task_id.as_str())
        .bind(request.idempotency_key.trim())
        .bind(&submission_sha256)
        .bind(snapshot.snapshot_ref.authority_key().as_str())
        .bind(snapshot.snapshot_ref.resource_id())
        .bind(snapshot.snapshot_ref.resource_version())
        .bind(&snapshot.content_sha256)
        .bind(snapshot.valid_until)
        .bind(snapshot.organization_ref.authority_key().as_str())
        .bind(snapshot.organization_ref.resource_id())
        .bind(snapshot.organization_ref.resource_version())
        .bind(snapshot.project_ref.authority_key().as_str())
        .bind(snapshot.project_ref.resource_id())
        .bind(snapshot.project_ref.resource_version())
        .bind(snapshot.rbac_policy_version_ref.resource_id())
        .bind(snapshot.rbac_policy_version_ref.resource_version())
        .bind(snapshot.quota_policy_version_ref.resource_id())
        .bind(snapshot.quota_policy_version_ref.resource_version())
        .bind(u64_to_i64(snapshot.policy_revocation_epoch, "policy revocation epoch")?)
        .bind(placement_policy_json)
        .bind(request.requirements.resource_class.trim())
        .bind(capabilities_json)
        .bind(u32_to_i64(request.requirements.quota_max_active))
        .bind(request.requirements.priority)
        .bind(u32_to_i64(request.requirements.max_attempts))
        .bind(request.submitted_at)
        .bind(request.submitted_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        append_outbox(
            &mut connection,
            "task.submitted",
            &request.execution_task_id,
            None,
            &submission_sha256,
            request.submitted_at,
        )
        .await?;
        commit(&mut connection).await?;
        get_task(&self.pool, &request.execution_task_id)
            .await?
            .ok_or_else(|| FleetSchedulerError::Unavailable("submitted task missing".to_string()))
    }

    async fn heartbeat(
        &self,
        request: &FleetHeartbeatRequest,
    ) -> Result<WorkerAvailability, FleetSchedulerError> {
        validate_heartbeat(request)?;
        let availability = &request.availability;
        let mut connection = begin_immediate(&self.pool).await?;
        validate_current_identity(&mut connection, &request.workload).await?;
        let worker = sqlx::query(
            "SELECT worker.status, worker.trust_domain, worker.labels_json, incarnation.is_current \
             FROM worker_incarnations AS incarnation \
             JOIN workers AS worker ON worker.id = incarnation.worker_id \
             WHERE incarnation.id = ? AND incarnation.worker_id = ?",
        )
        .bind(availability.worker_incarnation_id.as_str())
        .bind(availability.worker_id.as_str())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| FleetSchedulerError::Denied(FleetDenialReason::WorkerNotCurrent))?;
        if worker.get::<i64, _>("is_current") != 1 {
            rollback(&mut connection).await?;
            return Err(FleetSchedulerError::Denied(
                FleetDenialReason::WorkerNotCurrent,
            ));
        }
        let durable_status = worker.get::<String, _>("status");
        if durable_status == "retired" {
            rollback(&mut connection).await?;
            return Err(FleetSchedulerError::Denied(
                FleetDenialReason::WorkerNotOnline,
            ));
        }
        if worker.get::<String, _>("trust_domain") != request.workload.trust_domain {
            rollback(&mut connection).await?;
            return Err(FleetSchedulerError::Denied(
                FleetDenialReason::IdentityMismatch,
            ));
        }
        validate_enrollment_attestation(
            &mut connection,
            &worker.get::<String, _>("labels_json"),
            availability,
            &request.workload.trust_domain,
        )
        .await?;

        if let Some(row) = sqlx::query(
            "SELECT heartbeat_sequence, worker_id FROM enterprise_worker_availability \
             WHERE worker_incarnation_id = ?",
        )
        .bind(availability.worker_incarnation_id.as_str())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?
        {
            let durable_sequence = row.get::<i64, _>("heartbeat_sequence");
            let requested_sequence =
                u64_to_i64(availability.heartbeat_sequence, "heartbeat sequence")?;
            if requested_sequence <= durable_sequence {
                rollback(&mut connection).await?;
                return Err(FleetSchedulerError::Denied(
                    FleetDenialReason::HeartbeatSequenceRegressed,
                ));
            }
            if row.get::<String, _>("worker_id") != availability.worker_id.as_str() {
                rollback(&mut connection).await?;
                return Err(FleetSchedulerError::Denied(
                    FleetDenialReason::IdentityMismatch,
                ));
            }
        }

        let capabilities_json = json(&availability.capabilities)?;
        let classifications_json = json(&availability.data_classifications)?;
        let egress_json = json(&availability.egress_profile_ids)?;
        let cache_json = json(&availability.tenant_cache_namespaces)?;
        sqlx::query(
            "INSERT INTO enterprise_worker_availability (\
                worker_incarnation_id, worker_id, heartbeat_sequence, worker_status, \
                daemon_version, protocol_min, protocol_max, region, zone, resource_class, \
                capabilities_json, total_slots, available_slots, data_classifications_json, \
                image_digest, image_signature_verified, dedicated_pool, \
                egress_profile_ids_json, tenant_cache_namespaces_json, observed_at, updated_at\
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(worker_incarnation_id) DO UPDATE SET \
                heartbeat_sequence = excluded.heartbeat_sequence, \
                worker_status = excluded.worker_status, daemon_version = excluded.daemon_version, \
                protocol_min = excluded.protocol_min, protocol_max = excluded.protocol_max, \
                region = excluded.region, zone = excluded.zone, resource_class = excluded.resource_class, \
                capabilities_json = excluded.capabilities_json, total_slots = excluded.total_slots, \
                available_slots = excluded.available_slots, \
                data_classifications_json = excluded.data_classifications_json, \
                image_digest = excluded.image_digest, \
                image_signature_verified = excluded.image_signature_verified, \
                dedicated_pool = excluded.dedicated_pool, \
                egress_profile_ids_json = excluded.egress_profile_ids_json, \
                tenant_cache_namespaces_json = excluded.tenant_cache_namespaces_json, \
                observed_at = excluded.observed_at, updated_at = excluded.updated_at",
        )
        .bind(availability.worker_incarnation_id.as_str())
        .bind(availability.worker_id.as_str())
        .bind(u64_to_i64(availability.heartbeat_sequence, "heartbeat sequence")?)
        .bind(availability.worker_status.as_str())
        .bind(availability.daemon_version.trim())
        .bind(u32_to_i64(availability.protocol_min))
        .bind(u32_to_i64(availability.protocol_max))
        .bind(availability.region.trim())
        .bind(availability.zone.trim())
        .bind(availability.resource_class.trim())
        .bind(capabilities_json)
        .bind(u32_to_i64(availability.total_slots))
        .bind(u32_to_i64(availability.available_slots))
        .bind(classifications_json)
        .bind(&availability.image_digest)
        .bind(i64::from(availability.image_signature_verified))
        .bind(i64::from(availability.dedicated_pool))
        .bind(egress_json)
        .bind(cache_json)
        .bind(request.observed_at)
        .bind(request.observed_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        sqlx::query(
            "UPDATE worker_incarnations SET last_seen_at = ? WHERE id = ? AND is_current = 1",
        )
        .bind(request.observed_at)
        .bind(availability.worker_incarnation_id.as_str())
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        sqlx::query(
            "UPDATE workers SET status = ?, updated_at = ?, record_version = record_version + 1 \
             WHERE id = ? AND status <> 'retired'",
        )
        .bind(availability.worker_status.as_str())
        .bind(request.observed_at)
        .bind(availability.worker_id.as_str())
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(&mut connection).await?;
        Ok(availability.clone())
    }

    async fn pull(
        &self,
        request: &FleetPullRequest,
    ) -> Result<Option<FleetAssignment>, FleetSchedulerError> {
        validate_pull(request)?;
        let availability = load_availability(&self.pool, request).await?;
        if availability.worker_status == WorkerStatus::Draining {
            return Err(FleetSchedulerError::Denied(
                FleetDenialReason::WorkerDraining,
            ));
        }
        if availability.worker_status != WorkerStatus::Online {
            return Err(FleetSchedulerError::Denied(
                FleetDenialReason::WorkerNotOnline,
            ));
        }
        if availability.available_slots == 0 {
            return Err(FleetSchedulerError::Denied(
                FleetDenialReason::CapacityUnavailable,
            ));
        }
        if request.protocol_version < availability.protocol_min
            || request.protocol_version > availability.protocol_max
        {
            return Err(FleetSchedulerError::Denied(
                FleetDenialReason::ProtocolUnsupported,
            ));
        }
        let max_age = i64::from(request.heartbeat_max_age_seconds);
        let observed_at: i64 = sqlx::query_scalar(
            "SELECT observed_at FROM enterprise_worker_availability WHERE worker_incarnation_id = ?",
        )
        .bind(availability.worker_incarnation_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(storage_error)?;
        if request.observed_at.saturating_sub(observed_at) > max_age {
            return Err(FleetSchedulerError::Denied(
                FleetDenialReason::HeartbeatStale,
            ));
        }

        let rows = sqlx::query(
            "SELECT execution_task_id, snapshot_authority_key, snapshot_resource_id, \
                    snapshot_resource_version, snapshot_valid_until, organization_authority_key, \
                    organization_resource_id, organization_resource_version, project_authority_key, \
                    project_resource_id, project_resource_version, policy_revocation_epoch, \
                    placement_policy_json, resource_class, required_capabilities_json, \
                    quota_max_active \
             FROM enterprise_fleet_queue \
             WHERE (status = 'queued' OR (status = 'retry_wait' AND next_eligible_at <= ?)) \
             ORDER BY priority DESC, created_at, execution_task_id LIMIT 64",
        )
        .bind(request.observed_at)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?;

        for row in rows {
            let task_id = TaskRunId::from_string(row.get::<String, _>("execution_task_id"));
            let block =
                match candidate_block(&row, &availability, &request.workload, request.observed_at)?
                {
                    Some(block) => block,
                    None => {
                        let epoch_request = epoch_request_from_row(
                            &row,
                            SecurityCheckpoint::Dispatch,
                            request.observed_at,
                        )?;
                        match self.check_epoch(&epoch_request).await {
                            Ok(()) => {
                                if quota_available(&self.pool, &row).await? {
                                    return self
                                        .acquire_task(request, &availability, &task_id)
                                        .await
                                        .map(Some);
                                }
                                FleetDenialReason::QuotaExceeded
                            }
                            Err(FleetSchedulerError::Denied(reason)) => reason,
                            Err(error) => return Err(error),
                        }
                    }
                };
            record_block(
                &self.pool,
                &request.workload,
                &task_id,
                block,
                request.observed_at,
            )
            .await?;
        }
        Ok(None)
    }

    async fn renew(
        &self,
        request: &FleetRenewRequest,
    ) -> Result<TaskLeaseGrant, FleetSchedulerError> {
        validate_report_workload(&request.workload, &request.claim, request.observed_at)?;
        if request.expires_at <= request.observed_at
            || request.expires_at.saturating_sub(request.observed_at) > 300
        {
            return Err(FleetSchedulerError::Invalid(
                "renewal expiry must be in the future and at most 300 seconds".to_string(),
            ));
        }
        let scope = queue_epoch_scope(&self.pool, &request.claim.execution_task_id).await?;
        if scope.snapshot_ref != request.snapshot_ref
            || scope.pinned_epoch != request.pinned_revocation_epoch
        {
            return Err(FleetSchedulerError::Denied(
                FleetDenialReason::RevocationEpochStale,
            ));
        }
        self.check_epoch(&scope.request(SecurityCheckpoint::LeaseRenewal, request.observed_at))
            .await?;
        let mut connection = begin_immediate(&self.pool).await?;
        validate_current_identity(&mut connection, &request.workload).await?;
        let lease_request = agentd_core::ports::TaskLeaseRenewRequest {
            claim: request.claim.clone(),
            observed_at: request.observed_at,
            expires_at: request.expires_at,
        };
        let decision = renew_in_transaction(&mut connection, &lease_request)
            .await
            .map_err(lease_error)?;
        let grant = match decision {
            Decision::Return(grant) => grant,
            Decision::Reject(error) => {
                record_fencing_rejection(
                    &mut connection,
                    "renew",
                    &request.claim,
                    &error.to_string(),
                    request.observed_at,
                )
                .await?;
                commit(&mut connection).await?;
                return Err(lease_error(error));
            }
        };
        let payload_sha256 = sha256(&grant)?;
        append_outbox(
            &mut connection,
            "lease.renewed",
            &request.claim.execution_task_id,
            Some(&request.claim),
            &payload_sha256,
            request.observed_at,
        )
        .await?;
        sqlx::query("UPDATE enterprise_fleet_queue SET updated_at = ? WHERE execution_task_id = ?")
            .bind(request.observed_at)
            .bind(request.claim.execution_task_id.as_str())
            .execute(&mut *connection)
            .await
            .map_err(storage_error)?;
        commit(&mut connection).await?;
        Ok(grant)
    }

    async fn complete(
        &self,
        request: &FleetCompletionReport,
    ) -> Result<FleetTaskRecord, FleetSchedulerError> {
        validate_report_workload(&request.workload, &request.claim, request.observed_at)?;
        validate_key(&request.idempotency_key, "completion idempotency key")?;
        validate_sha256(&request.outcome_sha256, "outcome sha256")?;
        let request_sha256 = sha256(request)?;
        let mut connection = begin_immediate(&self.pool).await?;
        validate_current_identity(&mut connection, &request.workload).await?;
        if receipt_matches(
            &mut connection,
            "complete",
            &request.idempotency_key,
            &request.claim,
            &request_sha256,
        )
        .await?
        {
            commit(&mut connection).await?;
            return required_task(&self.pool, &request.claim.execution_task_id).await;
        }
        let close_request = agentd_core::ports::TaskLeaseCloseRequest {
            claim: request.claim.clone(),
            observed_at: request.observed_at,
            reason: "worker_completed".to_string(),
        };
        match close_in_transaction(&mut connection, &close_request, LeaseStatus::Released)
            .await
            .map_err(lease_error)?
        {
            Decision::Return(_) => {}
            Decision::Reject(error) => {
                reject_report(
                    &mut connection,
                    "complete",
                    &request.claim,
                    error,
                    request.observed_at,
                )
                .await?;
            }
        }
        let updated = sqlx::query(
            "UPDATE enterprise_fleet_queue SET status = 'completed', outcome_sha256 = ?, \
             next_eligible_at = NULL, block_code = NULL, updated_at = ? \
             WHERE execution_task_id = ? AND status = 'acquired' AND assigned_lease_id = ?",
        )
        .bind(&request.outcome_sha256)
        .bind(request.observed_at)
        .bind(request.claim.execution_task_id.as_str())
        .bind(request.claim.lease_id.as_str())
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        if updated.rows_affected() != 1 {
            rollback(&mut connection).await?;
            return Err(FleetSchedulerError::Denied(FleetDenialReason::TaskTerminal));
        }
        release_slot(&mut connection, &request.claim.worker_incarnation_id).await?;
        insert_receipt(
            &mut connection,
            "complete",
            &request.idempotency_key,
            &request.claim,
            &request_sha256,
            request.observed_at,
        )
        .await?;
        append_outbox(
            &mut connection,
            "task.completed",
            &request.claim.execution_task_id,
            Some(&request.claim),
            &request_sha256,
            request.observed_at,
        )
        .await?;
        commit(&mut connection).await?;
        required_task(&self.pool, &request.claim.execution_task_id).await
    }

    async fn fail(
        &self,
        request: &FleetFailureReport,
    ) -> Result<FleetTaskRecord, FleetSchedulerError> {
        validate_report_workload(&request.workload, &request.claim, request.observed_at)?;
        validate_key(&request.idempotency_key, "failure idempotency key")?;
        validate_code(&request.failure_code, "failure code")?;
        let request_sha256 = sha256(request)?;
        let mut connection = begin_immediate(&self.pool).await?;
        validate_current_identity(&mut connection, &request.workload).await?;
        if receipt_matches(
            &mut connection,
            "fail",
            &request.idempotency_key,
            &request.claim,
            &request_sha256,
        )
        .await?
        {
            commit(&mut connection).await?;
            return required_task(&self.pool, &request.claim.execution_task_id).await;
        }
        let close_request = agentd_core::ports::TaskLeaseCloseRequest {
            claim: request.claim.clone(),
            observed_at: request.observed_at,
            reason: request.failure_code.clone(),
        };
        match close_in_transaction(&mut connection, &close_request, LeaseStatus::Released)
            .await
            .map_err(lease_error)?
        {
            Decision::Return(_) => {}
            Decision::Reject(error) => {
                reject_report(
                    &mut connection,
                    "fail",
                    &request.claim,
                    error,
                    request.observed_at,
                )
                .await?;
            }
        }
        let row = sqlx::query(
            "SELECT attempt_count, max_attempts FROM enterprise_fleet_queue \
             WHERE execution_task_id = ? AND status = 'acquired' AND assigned_lease_id = ?",
        )
        .bind(request.claim.execution_task_id.as_str())
        .bind(request.claim.lease_id.as_str())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| FleetSchedulerError::Denied(FleetDenialReason::TaskTerminal))?;
        let attempt_count = row.get::<i64, _>("attempt_count");
        let max_attempts = row.get::<i64, _>("max_attempts");
        let retry = request.retryable && attempt_count < max_attempts;
        let status = if retry { "retry_wait" } else { "dead_letter" };
        let next_eligible_at = retry.then(|| {
            request
                .observed_at
                .saturating_add(retry_delay_seconds(attempt_count))
        });
        sqlx::query(
            "UPDATE enterprise_fleet_queue SET status = ?, next_eligible_at = ?, block_code = ?, \
             updated_at = ? WHERE execution_task_id = ? AND status = 'acquired'",
        )
        .bind(status)
        .bind(next_eligible_at)
        .bind(request.failure_code.trim())
        .bind(request.observed_at)
        .bind(request.claim.execution_task_id.as_str())
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        release_slot(&mut connection, &request.claim.worker_incarnation_id).await?;
        insert_receipt(
            &mut connection,
            "fail",
            &request.idempotency_key,
            &request.claim,
            &request_sha256,
            request.observed_at,
        )
        .await?;
        append_outbox(
            &mut connection,
            if retry {
                "task.retry_wait"
            } else {
                "task.dead_letter"
            },
            &request.claim.execution_task_id,
            Some(&request.claim),
            &request_sha256,
            request.observed_at,
        )
        .await?;
        commit(&mut connection).await?;
        required_task(&self.pool, &request.claim.execution_task_id).await
    }

    async fn cancel(
        &self,
        request: &FleetCancelRequest,
    ) -> Result<FleetTaskRecord, FleetSchedulerError> {
        validate_report_workload(&request.workload, &request.claim, request.observed_at)?;
        validate_key(&request.idempotency_key, "cancel idempotency key")?;
        validate_code(&request.reason_code, "cancel reason code")?;
        let request_sha256 = sha256(request)?;
        let mut connection = begin_immediate(&self.pool).await?;
        validate_current_identity(&mut connection, &request.workload).await?;
        if receipt_matches(
            &mut connection,
            "cancel",
            &request.idempotency_key,
            &request.claim,
            &request_sha256,
        )
        .await?
        {
            commit(&mut connection).await?;
            return required_task(&self.pool, &request.claim.execution_task_id).await;
        }
        let close_request = agentd_core::ports::TaskLeaseCloseRequest {
            claim: request.claim.clone(),
            observed_at: request.observed_at,
            reason: request.reason_code.clone(),
        };
        match close_in_transaction(&mut connection, &close_request, LeaseStatus::Cancelled)
            .await
            .map_err(lease_error)?
        {
            Decision::Return(_) => {}
            Decision::Reject(error) => {
                reject_report(
                    &mut connection,
                    "cancel",
                    &request.claim,
                    error,
                    request.observed_at,
                )
                .await?;
            }
        }
        sqlx::query(
            "UPDATE enterprise_fleet_queue SET status = 'cancelled', next_eligible_at = NULL, \
             block_code = ?, updated_at = ? \
             WHERE execution_task_id = ? AND status = 'acquired' AND assigned_lease_id = ?",
        )
        .bind(request.reason_code.trim())
        .bind(request.observed_at)
        .bind(request.claim.execution_task_id.as_str())
        .bind(request.claim.lease_id.as_str())
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        release_slot(&mut connection, &request.claim.worker_incarnation_id).await?;
        insert_receipt(
            &mut connection,
            "cancel",
            &request.idempotency_key,
            &request.claim,
            &request_sha256,
            request.observed_at,
        )
        .await?;
        append_outbox(
            &mut connection,
            "task.cancelled",
            &request.claim.execution_task_id,
            Some(&request.claim),
            &request_sha256,
            request.observed_at,
        )
        .await?;
        commit(&mut connection).await?;
        required_task(&self.pool, &request.claim.execution_task_id).await
    }

    async fn acknowledge_artifact_upload(
        &self,
        request: &ArtifactUploadAckRequest,
    ) -> Result<ArtifactUploadAck, FleetSchedulerError> {
        validate_artifact_ack(request)?;
        let scope = queue_epoch_scope(&self.pool, &request.claim.execution_task_id).await?;
        self.check_epoch(
            &scope.request(SecurityCheckpoint::ArtifactAcceptance, request.observed_at),
        )
        .await?;
        let request_sha256 = sha256(request)?;
        let mut connection = begin_immediate(&self.pool).await?;
        validate_current_identity(&mut connection, &request.workload).await?;
        if receipt_matches(
            &mut connection,
            "artifact",
            &request.idempotency_key,
            &request.claim,
            &request_sha256,
        )
        .await?
        {
            let ack = get_artifact_ack(&mut connection, &request.upload_id).await?;
            commit(&mut connection).await?;
            return ack.ok_or_else(|| {
                FleetSchedulerError::Unavailable(
                    "artifact receipt exists without acknowledgement".to_string(),
                )
            });
        }
        authorize_or_reject(
            &mut connection,
            "artifact",
            &request.claim,
            request.observed_at,
        )
        .await?;
        sqlx::query(
            "INSERT INTO enterprise_artifact_upload_acknowledgements (\
                upload_id, execution_artifact_id, execution_task_id, worker_incarnation_id, \
                lease_id, fencing_token, idempotency_key, artifact_sha256, upload_attempt, \
                part_count, acknowledged_at\
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(request.upload_id.as_str())
        .bind(request.execution_artifact_id.as_str())
        .bind(request.claim.execution_task_id.as_str())
        .bind(request.claim.worker_incarnation_id.as_str())
        .bind(request.claim.lease_id.as_str())
        .bind(token_to_i64(request.claim.fencing_token)?)
        .bind(request.idempotency_key.trim())
        .bind(&request.artifact_sha256)
        .bind(u32_to_i64(request.upload_attempt))
        .bind(u32_to_i64(request.part_count))
        .bind(request.observed_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        insert_receipt(
            &mut connection,
            "artifact",
            &request.idempotency_key,
            &request.claim,
            &request_sha256,
            request.observed_at,
        )
        .await?;
        append_outbox(
            &mut connection,
            "artifact.upload_acknowledged",
            &request.claim.execution_task_id,
            Some(&request.claim),
            &request_sha256,
            request.observed_at,
        )
        .await?;
        let ack = get_artifact_ack(&mut connection, &request.upload_id)
            .await?
            .ok_or_else(|| {
                FleetSchedulerError::Unavailable("artifact acknowledgement missing".to_string())
            })?;
        commit(&mut connection).await?;
        Ok(ack)
    }

    async fn admit_side_effect(
        &self,
        request: &FleetSideEffectRequest,
    ) -> Result<FleetSideEffectAdmission, FleetSchedulerError> {
        validate_side_effect(request)?;
        let scope = queue_epoch_scope(&self.pool, &request.claim.execution_task_id).await?;
        self.check_epoch(&scope.request(request.checkpoint, request.observed_at))
            .await?;
        let request_sha256 = sha256(request)?;
        let admission = FleetSideEffectAdmission {
            execution_task_id: request.claim.execution_task_id.clone(),
            lease_id: request.claim.lease_id.clone(),
            fencing_token: request.claim.fencing_token,
            checkpoint: request.checkpoint,
            action: request.action,
            admitted_at: request.observed_at,
        };
        let mut connection = begin_immediate(&self.pool).await?;
        validate_current_identity(&mut connection, &request.workload).await?;
        if receipt_matches(
            &mut connection,
            "side_effect",
            &request.idempotency_key,
            &request.claim,
            &request_sha256,
        )
        .await?
        {
            commit(&mut connection).await?;
            return Ok(admission);
        }
        authorize_or_reject(
            &mut connection,
            "side_effect",
            &request.claim,
            request.observed_at,
        )
        .await?;
        sqlx::query(
            "INSERT INTO enterprise_side_effect_admissions (\
                idempotency_key, execution_task_id, worker_incarnation_id, lease_id, \
                fencing_token, checkpoint, action, admitted_at\
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(request.idempotency_key.trim())
        .bind(request.claim.execution_task_id.as_str())
        .bind(request.claim.worker_incarnation_id.as_str())
        .bind(request.claim.lease_id.as_str())
        .bind(token_to_i64(request.claim.fencing_token)?)
        .bind(request.checkpoint.as_str())
        .bind(request.action.as_str())
        .bind(request.observed_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        insert_receipt(
            &mut connection,
            "side_effect",
            &request.idempotency_key,
            &request.claim,
            &request_sha256,
            request.observed_at,
        )
        .await?;
        append_outbox(
            &mut connection,
            "side_effect.admitted",
            &request.claim.execution_task_id,
            Some(&request.claim),
            &request_sha256,
            request.observed_at,
        )
        .await?;
        commit(&mut connection).await?;
        Ok(admission)
    }

    async fn reap(
        &self,
        request: &FleetReapRequest,
    ) -> Result<FleetReapSummary, FleetSchedulerError> {
        validate_reap(request)?;
        let mut connection = begin_immediate(&self.pool).await?;
        let offlined = sqlx::query(
            "UPDATE workers SET status = 'offline', updated_at = ?, record_version = record_version + 1 \
             WHERE status IN ('online', 'draining') AND id IN (\
                 SELECT worker_id FROM enterprise_worker_availability \
                 WHERE observed_at < ? AND worker_status <> 'offline'\
             )",
        )
        .bind(request.observed_at)
        .bind(request.heartbeat_stale_before)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?
        .rows_affected();
        sqlx::query(
            "UPDATE enterprise_worker_availability SET worker_status = 'offline', updated_at = ? \
             WHERE observed_at < ? AND worker_status <> 'offline'",
        )
        .bind(request.observed_at)
        .bind(request.heartbeat_stale_before)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;

        let leases = sqlx::query(
            "SELECT lease.id, lease.execution_task_id, lease.worker_incarnation_id, \
                    lease.fencing_token, queue.attempt_count, queue.max_attempts \
             FROM execution_task_leases AS lease \
             JOIN enterprise_fleet_queue AS queue \
               ON queue.execution_task_id = lease.execution_task_id \
              AND queue.assigned_lease_id = lease.id \
             LEFT JOIN enterprise_worker_availability AS availability \
               ON availability.worker_incarnation_id = lease.worker_incarnation_id \
             WHERE lease.status = 'active' AND queue.status = 'acquired' AND (\
                 lease.expires_at <= ? OR availability.observed_at < ? \
                 OR availability.worker_status = 'offline' OR availability.worker_incarnation_id IS NULL\
             ) ORDER BY lease.execution_task_id, lease.fencing_token",
        )
        .bind(request.lease_expired_before)
        .bind(request.heartbeat_stale_before)
        .fetch_all(&mut *connection)
        .await
        .map_err(storage_error)?;
        let mut requeued = 0_u64;
        let mut dead_lettered = 0_u64;
        for row in &leases {
            let task_id = TaskRunId::from_string(row.get::<String, _>("execution_task_id"));
            let incarnation =
                WorkerIncarnationId::from_string(row.get::<String, _>("worker_incarnation_id"));
            let lease_id = LeaseId::from_string(row.get::<String, _>("id"));
            let token = fencing_from_i64(row.get::<i64, _>("fencing_token"))?;
            terminalize_active(
                &mut connection,
                task_id.as_str(),
                lease_id.as_str(),
                LeaseStatus::Expired,
                request.observed_at,
                "fleet_reaper",
            )
            .await
            .map_err(lease_error)?;
            let exhausted = row.get::<i64, _>("attempt_count") >= row.get::<i64, _>("max_attempts");
            let status = if exhausted { "dead_letter" } else { "queued" };
            if exhausted {
                dead_lettered += 1;
            } else {
                requeued += 1;
            }
            sqlx::query(
                "UPDATE enterprise_fleet_queue SET status = ?, next_eligible_at = NULL, \
                 block_code = 'worker_or_lease_stale', updated_at = ? \
                 WHERE execution_task_id = ? AND status = 'acquired' AND assigned_lease_id = ?",
            )
            .bind(status)
            .bind(request.observed_at)
            .bind(task_id.as_str())
            .bind(lease_id.as_str())
            .execute(&mut *connection)
            .await
            .map_err(storage_error)?;
            let claim = TaskLeaseClaim {
                execution_task_id: task_id.clone(),
                worker_incarnation_id: incarnation,
                lease_id,
                fencing_token: token,
            };
            let payload_sha256 = sha256(&claim)?;
            append_outbox(
                &mut connection,
                if exhausted {
                    "task.dead_letter"
                } else {
                    "task.requeued"
                },
                &task_id,
                Some(&claim),
                &payload_sha256,
                request.observed_at,
            )
            .await?;
        }
        commit(&mut connection).await?;
        Ok(FleetReapSummary {
            workers_offlined: offlined,
            leases_expired: u64::try_from(leases.len()).map_err(|_| {
                FleetSchedulerError::Unavailable("reaper result overflow".to_string())
            })?,
            tasks_requeued: requeued,
            tasks_dead_lettered: dead_lettered,
        })
    }

    async fn outbox_after(
        &self,
        after: Option<&FleetOutboxId>,
        limit: u32,
    ) -> Result<Vec<FleetOutboxEvent>, FleetSchedulerError> {
        if limit == 0 || limit > 1_000 {
            return Err(FleetSchedulerError::Invalid(
                "outbox limit must be within 1..=1000".to_string(),
            ));
        }
        let after_sequence = if let Some(after) = after {
            validate_id(after.as_str(), "fo_", "outbox event id")?;
            sqlx::query_scalar::<_, i64>(
                "SELECT sequence FROM enterprise_scheduler_outbox WHERE id = ?",
            )
            .bind(after.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .ok_or_else(|| FleetSchedulerError::NotFound(format!("outbox event {after}")))?
        } else {
            0
        };
        let rows = sqlx::query(
            "SELECT id, event_type, execution_task_id, worker_incarnation_id, lease_id, \
                    fencing_token, payload_sha256, created_at, delivered_at \
             FROM enterprise_scheduler_outbox WHERE sequence > ? ORDER BY sequence LIMIT ?",
        )
        .bind(after_sequence)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?;
        rows.iter().map(row_to_outbox).collect()
    }

    async fn explain(
        &self,
        execution_task_id: &TaskRunId,
    ) -> Result<Option<FleetExplain>, FleetSchedulerError> {
        validate_id(execution_task_id.as_str(), "tr_", "execution task id")?;
        Ok(get_task(&self.pool, execution_task_id)
            .await?
            .map(|task| FleetExplain {
                execution_task_id: task.execution_task_id,
                status: task.status,
                attempt_count: task.attempt_count,
                worker_incarnation_id: task
                    .current_claim
                    .as_ref()
                    .map(|claim| claim.worker_incarnation_id.clone()),
                current_claim: task.current_claim,
                snapshot_ref: task.snapshot_ref,
                policy_revocation_epoch: task.policy_revocation_epoch,
                block_code: task.block_code,
                next_eligible_at: task.next_eligible_at,
                updated_at: task.updated_at,
            }))
    }
}

impl SqliteFleetScheduler {
    async fn acquire_task(
        &self,
        request: &FleetPullRequest,
        availability: &WorkerAvailability,
        task_id: &TaskRunId,
    ) -> Result<FleetAssignment, FleetSchedulerError> {
        let incarnation =
            request
                .workload
                .worker_incarnation_id
                .as_ref()
                .ok_or(FleetSchedulerError::Denied(
                    FleetDenialReason::IdentityMismatch,
                ))?;
        let mut connection = begin_immediate(&self.pool).await?;
        validate_current_identity(&mut connection, &request.workload).await?;
        let queue = sqlx::query(
            "SELECT status, next_eligible_at, project_authority_key, project_resource_id, \
                    project_resource_version, quota_max_active \
             FROM enterprise_fleet_queue WHERE execution_task_id = ?",
        )
        .bind(task_id.as_str())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| FleetSchedulerError::NotFound(format!("task {task_id}")))?;
        let status = queue.get::<String, _>("status");
        let eligible = status == "queued"
            || (status == "retry_wait"
                && queue
                    .get::<Option<i64>, _>("next_eligible_at")
                    .is_some_and(|at| at <= request.observed_at));
        if !eligible {
            rollback(&mut connection).await?;
            return Err(FleetSchedulerError::Conflict(
                "task changed before acquisition".to_string(),
            ));
        }
        if !quota_available_connection(&mut connection, &queue).await? {
            rollback(&mut connection).await?;
            return Err(FleetSchedulerError::Denied(
                FleetDenialReason::QuotaExceeded,
            ));
        }
        let claimed = sqlx::query(
            "UPDATE enterprise_worker_availability \
             SET available_slots = available_slots - 1, updated_at = ? \
             WHERE worker_incarnation_id = ? AND worker_status = 'online' \
               AND available_slots > 0 AND heartbeat_sequence = ?",
        )
        .bind(request.observed_at)
        .bind(incarnation.as_str())
        .bind(u64_to_i64(
            availability.heartbeat_sequence,
            "heartbeat sequence",
        )?)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        if claimed.rows_affected() != 1 {
            rollback(&mut connection).await?;
            return Err(FleetSchedulerError::Denied(
                FleetDenialReason::CapacityUnavailable,
            ));
        }
        let lease = dispatch_in_transaction(
            &mut connection,
            &TaskLeaseDispatchRequest {
                execution_task_id: task_id.clone(),
                worker_incarnation_id: incarnation.clone(),
                observed_at: request.observed_at,
                expires_at: request.lease_expires_at,
            },
        )
        .await
        .map_err(lease_error)?;
        let updated = sqlx::query(
            "UPDATE enterprise_fleet_queue SET status = 'acquired', \
             attempt_count = attempt_count + 1, assigned_lease_id = ?, \
             assigned_worker_incarnation_id = ?, next_eligible_at = NULL, block_code = NULL, \
             updated_at = ? WHERE execution_task_id = ? AND status IN ('queued', 'retry_wait')",
        )
        .bind(lease.lease_id.as_str())
        .bind(incarnation.as_str())
        .bind(request.observed_at)
        .bind(task_id.as_str())
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        if updated.rows_affected() != 1 {
            rollback(&mut connection).await?;
            return Err(FleetSchedulerError::Conflict(
                "task changed during acquisition".to_string(),
            ));
        }
        let payload_sha256 = sha256(&lease)?;
        append_outbox(
            &mut connection,
            "task.acquired",
            task_id,
            Some(&lease.claim()),
            &payload_sha256,
            request.observed_at,
        )
        .await?;
        commit(&mut connection).await?;
        let task = required_task(&self.pool, task_id).await?;
        Ok(FleetAssignment { task, lease })
    }
}

#[derive(Debug)]
struct QueueEpochScope {
    organization_ref: OrganizationRef,
    project_ref: ProjectRef,
    snapshot_ref: ProjectExecutionSnapshotRef,
    pinned_epoch: u64,
}

impl QueueEpochScope {
    fn request(&self, checkpoint: SecurityCheckpoint, observed_at: i64) -> SecurityEpochRequest {
        SecurityEpochRequest {
            checkpoint,
            organization_ref: self.organization_ref.clone(),
            project_ref: self.project_ref.clone(),
            execution_snapshot_ref: self.snapshot_ref.clone(),
            pinned_epoch: self.pinned_epoch,
            observed_at,
        }
    }
}

async fn queue_epoch_scope(
    pool: &SqlitePool,
    task_id: &TaskRunId,
) -> Result<QueueEpochScope, FleetSchedulerError> {
    let row = sqlx::query(
        "SELECT snapshot_authority_key, snapshot_resource_id, snapshot_resource_version, \
                organization_authority_key, organization_resource_id, organization_resource_version, \
                project_authority_key, project_resource_id, project_resource_version, \
                policy_revocation_epoch FROM enterprise_fleet_queue WHERE execution_task_id = ?",
    )
    .bind(task_id.as_str())
    .fetch_optional(pool)
    .await
    .map_err(storage_error)?
    .ok_or_else(|| FleetSchedulerError::NotFound(format!("task {task_id}")))?;
    epoch_scope_from_row(&row)
}

fn epoch_request_from_row(
    row: &sqlx::sqlite::SqliteRow,
    checkpoint: SecurityCheckpoint,
    observed_at: i64,
) -> Result<SecurityEpochRequest, FleetSchedulerError> {
    Ok(epoch_scope_from_row(row)?.request(checkpoint, observed_at))
}

fn epoch_scope_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<QueueEpochScope, FleetSchedulerError> {
    let snapshot_authority = authority(row, "snapshot_authority_key")?;
    let organization_authority = authority(row, "organization_authority_key")?;
    let project_authority = authority(row, "project_authority_key")?;
    Ok(QueueEpochScope {
        organization_ref: OrganizationRef::new(
            organization_authority,
            row.get::<String, _>("organization_resource_id"),
            row.get::<String, _>("organization_resource_version"),
        )
        .map_err(authority_error)?,
        project_ref: ProjectRef::new(
            project_authority,
            row.get::<String, _>("project_resource_id"),
            row.get::<String, _>("project_resource_version"),
        )
        .map_err(authority_error)?,
        snapshot_ref: ProjectExecutionSnapshotRef::new(
            snapshot_authority,
            row.get::<String, _>("snapshot_resource_id"),
            row.get::<String, _>("snapshot_resource_version"),
        )
        .map_err(authority_error)?,
        pinned_epoch: i64_to_u64(row.get::<i64, _>("policy_revocation_epoch"), "epoch")?,
    })
}

fn candidate_block(
    row: &sqlx::sqlite::SqliteRow,
    availability: &WorkerAvailability,
    workload: &agentd_core::types::AuthenticatedWorkload,
    observed_at: i64,
) -> Result<Option<FleetDenialReason>, FleetSchedulerError> {
    if row.get::<i64, _>("snapshot_valid_until") <= observed_at {
        return Ok(Some(FleetDenialReason::SnapshotExpired));
    }
    if row.get::<String, _>("resource_class") != availability.resource_class {
        return Ok(Some(FleetDenialReason::CapacityUnavailable));
    }
    let required: BTreeSet<String> =
        serde_json::from_str(&row.get::<String, _>("required_capabilities_json"))
            .map_err(|error| FleetSchedulerError::Unavailable(error.to_string()))?;
    if !required.is_subset(&availability.capabilities) {
        return Ok(Some(FleetDenialReason::CapacityUnavailable));
    }
    let policy: PlacementPolicy =
        serde_json::from_str(&row.get::<String, _>("placement_policy_json"))
            .map_err(|error| FleetSchedulerError::Unavailable(error.to_string()))?;
    if !availability
        .egress_profile_ids
        .contains(&policy.egress_profile_id)
        || !availability
            .tenant_cache_namespaces
            .contains(&policy.tenant_cache_namespace)
    {
        return Ok(Some(FleetDenialReason::PlacementDenied));
    }
    let candidate = PlacementCandidate {
        supported_data_classifications: availability.data_classifications.clone(),
        region: availability.region.clone(),
        worker_trust_domain: workload.trust_domain.clone(),
        image_digest: availability.image_digest.clone(),
        image_signature_verified: availability.image_signature_verified,
        dedicated_pool: availability.dedicated_pool,
        egress_profile_id: policy.egress_profile_id.clone(),
        tenant_cache_namespace: policy.tenant_cache_namespace.clone(),
    };
    if policy.evaluate(&candidate).is_err() {
        return Ok(Some(FleetDenialReason::PlacementDenied));
    }
    Ok(None)
}

async fn validate_enrollment_attestation(
    connection: &mut SqliteConnection,
    labels_json: &str,
    availability: &WorkerAvailability,
    trust_domain: &str,
) -> Result<(), FleetSchedulerError> {
    let labels: serde_json::Value = serde_json::from_str(labels_json)
        .map_err(|error| FleetSchedulerError::Unavailable(error.to_string()))?;
    let Some(attestation) = labels.get("agentd_attestation") else {
        return Err(FleetSchedulerError::Denied(
            FleetDenialReason::PlacementDenied,
        ));
    };
    let expected = |field: &str| attestation.get(field).and_then(serde_json::Value::as_str);
    let digest = |field: &str| expected(field).is_some_and(is_sha256);
    let Some(rollout_id) = expected("rollout_id") else {
        return Err(FleetSchedulerError::Denied(
            FleetDenialReason::PlacementDenied,
        ));
    };
    validate_id(rollout_id, "ir_", "worker rollout id")?;
    let rollout = sqlx::query(
        "SELECT image_digest, signature_bundle_sha256, policy_sha256, \
                required_zones_json, status \
         FROM enterprise_worker_image_rollouts WHERE rollout_id = ?",
    )
    .bind(rollout_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(storage_error)?
    .ok_or(FleetSchedulerError::Denied(
        FleetDenialReason::PlacementDenied,
    ))?;
    let required_zones: BTreeSet<String> =
        serde_json::from_str(&rollout.get::<String, _>("required_zones_json"))
            .map_err(|error| FleetSchedulerError::Unavailable(error.to_string()))?;
    let policy_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM enterprise_zone_pool_policies \
         WHERE rollout_id = ? AND region = ? AND zone = ? AND resource_class = ? \
           AND trust_domain = ? AND enabled = 1",
    )
    .bind(rollout_id)
    .bind(&availability.region)
    .bind(&availability.zone)
    .bind(&availability.resource_class)
    .bind(trust_domain)
    .fetch_one(&mut *connection)
    .await
    .map_err(storage_error)?;
    let rollout_status = rollout.get::<String, _>("status");
    let rollout_image = rollout.get::<String, _>("image_digest");
    let rollout_signature = rollout.get::<String, _>("signature_bundle_sha256");
    let rollout_policy = rollout.get::<String, _>("policy_sha256");
    let valid = availability.image_signature_verified
        && rollout_status != "rolled_back"
        && rollout_image.as_str() == availability.image_digest.as_str()
        && expected("signature_bundle_sha256") == Some(rollout_signature.as_str())
        && expected("signature_policy_sha256") == Some(rollout_policy.as_str())
        && expected("image_digest") == Some(availability.image_digest.as_str())
        && expected("region") == Some(availability.region.as_str())
        && expected("zone") == Some(availability.zone.as_str())
        && expected("resource_class") == Some(availability.resource_class.as_str())
        && digest("signature_bundle_sha256")
        && digest("signature_policy_sha256")
        && required_zones.contains(&availability.zone)
        && policy_count == 1;
    if valid {
        Ok(())
    } else {
        Err(FleetSchedulerError::Denied(
            FleetDenialReason::PlacementDenied,
        ))
    }
}

async fn quota_available(
    pool: &SqlitePool,
    row: &sqlx::sqlite::SqliteRow,
) -> Result<bool, FleetSchedulerError> {
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM enterprise_fleet_queue WHERE project_authority_key = ? \
         AND project_resource_id = ? AND project_resource_version = ? AND status = 'acquired'",
    )
    .bind(row.get::<String, _>("project_authority_key"))
    .bind(row.get::<String, _>("project_resource_id"))
    .bind(row.get::<String, _>("project_resource_version"))
    .fetch_one(pool)
    .await
    .map_err(storage_error)?;
    Ok(active < row.get::<i64, _>("quota_max_active"))
}

async fn quota_available_connection(
    connection: &mut SqliteConnection,
    row: &sqlx::sqlite::SqliteRow,
) -> Result<bool, FleetSchedulerError> {
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM enterprise_fleet_queue WHERE project_authority_key = ? \
         AND project_resource_id = ? AND project_resource_version = ? AND status = 'acquired'",
    )
    .bind(row.get::<String, _>("project_authority_key"))
    .bind(row.get::<String, _>("project_resource_id"))
    .bind(row.get::<String, _>("project_resource_version"))
    .fetch_one(&mut *connection)
    .await
    .map_err(storage_error)?;
    Ok(active < row.get::<i64, _>("quota_max_active"))
}

async fn load_availability(
    pool: &SqlitePool,
    request: &FleetPullRequest,
) -> Result<WorkerAvailability, FleetSchedulerError> {
    let worker_id = request
        .workload
        .worker_id
        .as_ref()
        .ok_or(FleetSchedulerError::Denied(
            FleetDenialReason::IdentityMismatch,
        ))?;
    let incarnation =
        request
            .workload
            .worker_incarnation_id
            .as_ref()
            .ok_or(FleetSchedulerError::Denied(
                FleetDenialReason::IdentityMismatch,
            ))?;
    let row = sqlx::query(
        "SELECT availability.worker_id, availability.worker_incarnation_id, \
                availability.heartbeat_sequence, availability.worker_status, \
                availability.daemon_version, availability.protocol_min, availability.protocol_max, \
                availability.region, availability.zone, availability.resource_class, \
                availability.capabilities_json, availability.total_slots, availability.available_slots, \
                availability.data_classifications_json, availability.image_digest, \
                availability.image_signature_verified, availability.dedicated_pool, \
                availability.egress_profile_ids_json, availability.tenant_cache_namespaces_json, \
                worker.status AS durable_worker_status, worker.trust_domain, incarnation.is_current \
         FROM enterprise_worker_availability AS availability \
         JOIN worker_incarnations AS incarnation ON incarnation.id = availability.worker_incarnation_id \
         JOIN workers AS worker ON worker.id = incarnation.worker_id \
         WHERE availability.worker_incarnation_id = ? AND availability.worker_id = ?",
    )
    .bind(incarnation.as_str())
    .bind(worker_id.as_str())
    .fetch_optional(pool)
    .await
    .map_err(storage_error)?
    .ok_or(FleetSchedulerError::Denied(FleetDenialReason::WorkerNotCurrent))?;
    if row.get::<i64, _>("is_current") != 1 {
        return Err(FleetSchedulerError::Denied(
            FleetDenialReason::WorkerNotCurrent,
        ));
    }
    if row.get::<String, _>("trust_domain") != request.workload.trust_domain {
        return Err(FleetSchedulerError::Denied(
            FleetDenialReason::IdentityMismatch,
        ));
    }
    let durable_status = row.get::<String, _>("durable_worker_status");
    if durable_status == "draining" {
        return Err(FleetSchedulerError::Denied(
            FleetDenialReason::WorkerDraining,
        ));
    }
    if durable_status != "online" {
        return Err(FleetSchedulerError::Denied(
            FleetDenialReason::WorkerNotOnline,
        ));
    }
    row_to_availability(&row)
}

fn row_to_availability(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<WorkerAvailability, FleetSchedulerError> {
    let status =
        WorkerStatus::try_from(row.get::<String, _>("worker_status").as_str()).map_err(|_| {
            FleetSchedulerError::Unavailable("invalid worker availability status".to_string())
        })?;
    Ok(WorkerAvailability {
        worker_id: WorkerId::from_string(row.get::<String, _>("worker_id")),
        worker_incarnation_id: WorkerIncarnationId::from_string(
            row.get::<String, _>("worker_incarnation_id"),
        ),
        heartbeat_sequence: i64_to_u64(row.get("heartbeat_sequence"), "heartbeat sequence")?,
        worker_status: status,
        daemon_version: row.get("daemon_version"),
        protocol_min: i64_to_u32(row.get("protocol_min"), "protocol min")?,
        protocol_max: i64_to_u32(row.get("protocol_max"), "protocol max")?,
        region: row.get("region"),
        zone: row.get("zone"),
        resource_class: row.get("resource_class"),
        capabilities: parse_json(row, "capabilities_json")?,
        total_slots: i64_to_u32(row.get("total_slots"), "total slots")?,
        available_slots: i64_to_u32(row.get("available_slots"), "available slots")?,
        data_classifications: parse_json(row, "data_classifications_json")?,
        image_digest: row.get("image_digest"),
        image_signature_verified: row.get::<i64, _>("image_signature_verified") == 1,
        dedicated_pool: row.get::<i64, _>("dedicated_pool") == 1,
        egress_profile_ids: parse_json(row, "egress_profile_ids_json")?,
        tenant_cache_namespaces: parse_json(row, "tenant_cache_namespaces_json")?,
    })
}

async fn get_task(
    pool: &SqlitePool,
    task_id: &TaskRunId,
) -> Result<Option<FleetTaskRecord>, FleetSchedulerError> {
    let query = format!("{TASK_SELECT} WHERE queue.execution_task_id = ?");
    let row = sqlx::query(&query)
        .bind(task_id.as_str())
        .fetch_optional(pool)
        .await
        .map_err(storage_error)?;
    row.as_ref().map(row_to_task).transpose()
}

async fn required_task(
    pool: &SqlitePool,
    task_id: &TaskRunId,
) -> Result<FleetTaskRecord, FleetSchedulerError> {
    get_task(pool, task_id)
        .await?
        .ok_or_else(|| FleetSchedulerError::NotFound(format!("task {task_id}")))
}

fn row_to_task(row: &sqlx::sqlite::SqliteRow) -> Result<FleetTaskRecord, FleetSchedulerError> {
    let status_text = row.get::<String, _>("status");
    let status = FleetQueueStatus::try_from(status_text.as_str()).map_err(|_| {
        FleetSchedulerError::Unavailable(format!("invalid queue status {status_text}"))
    })?;
    let snapshot_ref = ProjectExecutionSnapshotRef::new(
        authority(row, "snapshot_authority_key")?,
        row.get::<String, _>("snapshot_resource_id"),
        row.get::<String, _>("snapshot_resource_version"),
    )
    .map_err(authority_error)?;
    let current_claim = if status == FleetQueueStatus::Acquired {
        match (
            row.get::<Option<String>, _>("assigned_lease_id"),
            row.get::<Option<String>, _>("assigned_worker_incarnation_id"),
            row.get::<Option<i64>, _>("fencing_token"),
        ) {
            (Some(lease), Some(worker), Some(token)) => Some(TaskLeaseClaim {
                execution_task_id: TaskRunId::from_string(
                    row.get::<String, _>("execution_task_id"),
                ),
                worker_incarnation_id: WorkerIncarnationId::from_string(worker),
                lease_id: LeaseId::from_string(lease),
                fencing_token: fencing_from_i64(token)?,
            }),
            _ => {
                return Err(FleetSchedulerError::Unavailable(
                    "acquired queue record has no complete lease claim".to_string(),
                ));
            }
        }
    } else {
        None
    };
    Ok(FleetTaskRecord {
        execution_task_id: TaskRunId::from_string(row.get::<String, _>("execution_task_id")),
        idempotency_key: row.get("idempotency_key"),
        snapshot_ref,
        snapshot_content_sha256: row.get("snapshot_content_sha256"),
        policy_revocation_epoch: i64_to_u64(row.get("policy_revocation_epoch"), "epoch")?,
        requirements: FleetTaskRequirements {
            resource_class: row.get("resource_class"),
            required_capabilities: parse_json(row, "required_capabilities_json")?,
            quota_max_active: i64_to_u32(row.get("quota_max_active"), "quota max active")?,
            priority: row.get("priority"),
            max_attempts: i64_to_u32(row.get("max_attempts"), "max attempts")?,
        },
        status,
        attempt_count: i64_to_u32(row.get("attempt_count"), "attempt count")?,
        current_claim,
        next_eligible_at: row.get("next_eligible_at"),
        outcome_sha256: row.get("outcome_sha256"),
        block_code: row.get("block_code"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

async fn receipt_matches(
    connection: &mut SqliteConnection,
    kind: &str,
    idempotency_key: &str,
    claim: &TaskLeaseClaim,
    request_sha256: &str,
) -> Result<bool, FleetSchedulerError> {
    let row = sqlx::query(
        "SELECT execution_task_id, worker_incarnation_id, lease_id, fencing_token, request_sha256 \
         FROM enterprise_scheduler_report_receipts WHERE report_kind = ? AND idempotency_key = ?",
    )
    .bind(kind)
    .bind(idempotency_key.trim())
    .fetch_optional(&mut *connection)
    .await
    .map_err(storage_error)?;
    let Some(row) = row else {
        return Ok(false);
    };
    let matches = row.get::<String, _>("execution_task_id") == claim.execution_task_id.as_str()
        && row.get::<String, _>("worker_incarnation_id") == claim.worker_incarnation_id.as_str()
        && row.get::<String, _>("lease_id") == claim.lease_id.as_str()
        && row.get::<i64, _>("fencing_token") == token_to_i64(claim.fencing_token)?
        && row.get::<String, _>("request_sha256") == request_sha256;
    if !matches {
        return Err(FleetSchedulerError::Denied(
            FleetDenialReason::DuplicateMismatch,
        ));
    }
    Ok(true)
}

async fn insert_receipt(
    connection: &mut SqliteConnection,
    kind: &str,
    idempotency_key: &str,
    claim: &TaskLeaseClaim,
    request_sha256: &str,
    observed_at: i64,
) -> Result<(), FleetSchedulerError> {
    sqlx::query(
        "INSERT INTO enterprise_scheduler_report_receipts (\
            report_kind, idempotency_key, execution_task_id, worker_incarnation_id, \
            lease_id, fencing_token, request_sha256, recorded_at\
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(kind)
    .bind(idempotency_key.trim())
    .bind(claim.execution_task_id.as_str())
    .bind(claim.worker_incarnation_id.as_str())
    .bind(claim.lease_id.as_str())
    .bind(token_to_i64(claim.fencing_token)?)
    .bind(request_sha256)
    .bind(observed_at)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    Ok(())
}

async fn authorize_or_reject(
    connection: &mut SqliteImmediateTransaction,
    kind: &str,
    claim: &TaskLeaseClaim,
    observed_at: i64,
) -> Result<TaskLeaseGrant, FleetSchedulerError> {
    match authorize_claim(&mut **connection, claim, observed_at)
        .await
        .map_err(lease_error)?
    {
        ClaimAuthorization::Authorized(grant) => Ok(grant),
        ClaimAuthorization::Rejected(error) => {
            reject_report(connection, kind, claim, error, observed_at).await
        }
    }
}

async fn reject_report<T>(
    connection: &mut SqliteImmediateTransaction,
    kind: &str,
    claim: &TaskLeaseClaim,
    error: agentd_core::ports::TaskLeaseError,
    observed_at: i64,
) -> Result<T, FleetSchedulerError> {
    record_fencing_rejection(
        &mut **connection,
        kind,
        claim,
        &error.to_string(),
        observed_at,
    )
    .await?;
    commit(connection).await?;
    Err(lease_error(error))
}

async fn record_fencing_rejection(
    connection: &mut SqliteConnection,
    kind: &str,
    claim: &TaskLeaseClaim,
    _detail: &str,
    observed_at: i64,
) -> Result<(), FleetSchedulerError> {
    sqlx::query(
        "INSERT INTO enterprise_fencing_rejections (\
            report_kind, execution_task_id, worker_incarnation_id, lease_id, fencing_token, \
            denial_code, observed_at\
         ) VALUES (?, ?, ?, ?, ?, 'stale_fencing_token', ?)",
    )
    .bind(kind)
    .bind(claim.execution_task_id.as_str())
    .bind(claim.worker_incarnation_id.as_str())
    .bind(claim.lease_id.as_str())
    .bind(token_to_i64(claim.fencing_token)?)
    .bind(observed_at)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    Ok(())
}

async fn append_outbox(
    connection: &mut SqliteConnection,
    event_type: &str,
    task_id: &TaskRunId,
    claim: Option<&TaskLeaseClaim>,
    payload_sha256: &str,
    created_at: i64,
) -> Result<FleetOutboxId, FleetSchedulerError> {
    let id = FleetOutboxId::new();
    let (worker, lease, token) = claim.map_or((None, None, None), |claim| {
        (
            Some(claim.worker_incarnation_id.as_str()),
            Some(claim.lease_id.as_str()),
            Some(claim.fencing_token),
        )
    });
    sqlx::query(
        "INSERT INTO enterprise_scheduler_outbox (\
            id, event_type, execution_task_id, worker_incarnation_id, lease_id, fencing_token, \
            payload_sha256, created_at\
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.as_str())
    .bind(event_type)
    .bind(task_id.as_str())
    .bind(worker)
    .bind(lease)
    .bind(token.map(token_to_i64).transpose()?)
    .bind(payload_sha256)
    .bind(created_at)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    Ok(id)
}

fn row_to_outbox(row: &sqlx::sqlite::SqliteRow) -> Result<FleetOutboxEvent, FleetSchedulerError> {
    let task_id = TaskRunId::from_string(row.get::<String, _>("execution_task_id"));
    let claim = match (
        row.get::<Option<String>, _>("worker_incarnation_id"),
        row.get::<Option<String>, _>("lease_id"),
        row.get::<Option<i64>, _>("fencing_token"),
    ) {
        (None, None, None) => None,
        (Some(worker), Some(lease), Some(token)) => Some(TaskLeaseClaim {
            execution_task_id: task_id.clone(),
            worker_incarnation_id: WorkerIncarnationId::from_string(worker),
            lease_id: LeaseId::from_string(lease),
            fencing_token: fencing_from_i64(token)?,
        }),
        _ => {
            return Err(FleetSchedulerError::Unavailable(
                "outbox claim tuple is incomplete".to_string(),
            ));
        }
    };
    Ok(FleetOutboxEvent {
        id: FleetOutboxId::from_string(row.get::<String, _>("id")),
        event_type: row.get("event_type"),
        execution_task_id: task_id,
        claim,
        payload_sha256: row.get("payload_sha256"),
        created_at: row.get("created_at"),
        delivered_at: row.get("delivered_at"),
    })
}

async fn get_artifact_ack(
    connection: &mut SqliteConnection,
    upload_id: &ArtifactUploadId,
) -> Result<Option<ArtifactUploadAck>, FleetSchedulerError> {
    let row = sqlx::query(
        "SELECT upload_id, execution_artifact_id, execution_task_id, worker_incarnation_id, \
                lease_id, fencing_token, artifact_sha256, upload_attempt, part_count, acknowledged_at \
         FROM enterprise_artifact_upload_acknowledgements WHERE upload_id = ?",
    )
    .bind(upload_id.as_str())
    .fetch_optional(&mut *connection)
    .await
    .map_err(storage_error)?;
    row.map(|row| {
        let task_id = TaskRunId::from_string(row.get::<String, _>("execution_task_id"));
        Ok(ArtifactUploadAck {
            upload_id: ArtifactUploadId::from_string(row.get::<String, _>("upload_id")),
            execution_artifact_id: ExecutionArtifactId::from_string(
                row.get::<String, _>("execution_artifact_id"),
            ),
            claim: TaskLeaseClaim {
                execution_task_id: task_id,
                worker_incarnation_id: WorkerIncarnationId::from_string(
                    row.get::<String, _>("worker_incarnation_id"),
                ),
                lease_id: LeaseId::from_string(row.get::<String, _>("lease_id")),
                fencing_token: fencing_from_i64(row.get("fencing_token"))?,
            },
            artifact_sha256: row.get("artifact_sha256"),
            upload_attempt: i64_to_u32(row.get("upload_attempt"), "upload attempt")?,
            part_count: i64_to_u32(row.get("part_count"), "part count")?,
            acknowledged_at: row.get("acknowledged_at"),
        })
    })
    .transpose()
}

async fn release_slot(
    connection: &mut SqliteConnection,
    incarnation: &WorkerIncarnationId,
) -> Result<(), FleetSchedulerError> {
    sqlx::query(
        "UPDATE enterprise_worker_availability \
         SET available_slots = MIN(total_slots, available_slots + 1) \
         WHERE worker_incarnation_id = ?",
    )
    .bind(incarnation.as_str())
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    Ok(())
}

async fn record_block(
    pool: &SqlitePool,
    workload: &AuthenticatedWorkload,
    task_id: &TaskRunId,
    reason: FleetDenialReason,
    observed_at: i64,
) -> Result<(), FleetSchedulerError> {
    let mut connection = begin_immediate(pool).await?;
    validate_current_identity(&mut connection, workload).await?;
    sqlx::query(
        "UPDATE enterprise_fleet_queue SET block_code = ?, updated_at = ? \
         WHERE execution_task_id = ? AND status IN ('queued', 'retry_wait')",
    )
    .bind(reason.as_str())
    .bind(observed_at)
    .bind(task_id.as_str())
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    commit(&mut connection).await?;
    Ok(())
}

async fn validate_current_identity(
    connection: &mut SqliteConnection,
    workload: &AuthenticatedWorkload,
) -> Result<(), FleetSchedulerError> {
    let worker_id = workload
        .worker_id
        .as_ref()
        .ok_or(FleetSchedulerError::Denied(
            FleetDenialReason::IdentityMismatch,
        ))?;
    let incarnation_id =
        workload
            .worker_incarnation_id
            .as_ref()
            .ok_or(FleetSchedulerError::Denied(
                FleetDenialReason::IdentityMismatch,
            ))?;
    let active: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM workload_identity_bindings AS binding \
         JOIN worker_incarnations AS incarnation \
           ON incarnation.id = binding.worker_incarnation_id \
          AND incarnation.worker_id = binding.worker_id \
         JOIN workers AS worker ON worker.id = binding.worker_id \
         WHERE binding.certificate_sha256 = ? AND binding.spiffe_uri = ? \
           AND binding.role = 'worker' AND binding.trust_domain = ? \
           AND binding.worker_id = ? AND binding.worker_incarnation_id = ? \
           AND binding.not_before = ? AND binding.not_after = ? \
           AND binding.revoked_at IS NULL AND incarnation.is_current = 1 \
           AND worker.trust_domain = binding.trust_domain",
    )
    .bind(&workload.certificate_sha256)
    .bind(&workload.spiffe_uri)
    .bind(&workload.trust_domain)
    .bind(worker_id.as_str())
    .bind(incarnation_id.as_str())
    .bind(workload.not_before)
    .bind(workload.not_after)
    .fetch_optional(&mut *connection)
    .await
    .map_err(storage_error)?;
    if active.is_none() {
        return Err(FleetSchedulerError::Denied(
            FleetDenialReason::IdentityMismatch,
        ));
    }
    Ok(())
}

fn validate_submit(request: &FleetSubmitRequest) -> Result<(), FleetSchedulerError> {
    validate_id(
        request.execution_task_id.as_str(),
        "tr_",
        "execution task id",
    )?;
    validate_key(&request.idempotency_key, "submission idempotency key")?;
    request
        .snapshot
        .validate()
        .map_err(|error| FleetSchedulerError::Invalid(error.to_string()))?;
    if request.submitted_at < request.snapshot.issued_at
        || request.submitted_at >= request.snapshot.valid_until
    {
        return Err(FleetSchedulerError::Denied(
            FleetDenialReason::SnapshotExpired,
        ));
    }
    validate_key(&request.requirements.resource_class, "resource class")?;
    if request.requirements.max_attempts == 0
        || request.requirements.max_attempts > 10_000
        || request.requirements.quota_max_active == 0
        || request.requirements.quota_max_active > 100_000
        || request.requirements.required_capabilities.len() > MAX_FLEET_SET_VALUES
    {
        return Err(FleetSchedulerError::Invalid(
            "max attempts, quota, or capability count is outside protocol bounds".to_string(),
        ));
    }
    for capability in &request.requirements.required_capabilities {
        validate_code(capability, "required capability")?;
    }
    Ok(())
}

fn validate_heartbeat(request: &FleetHeartbeatRequest) -> Result<(), FleetSchedulerError> {
    validate_workload(
        &request.workload,
        &request.availability.worker_id,
        &request.availability.worker_incarnation_id,
        request.observed_at,
    )?;
    let availability = &request.availability;
    if availability.heartbeat_sequence == 0
        || availability.protocol_min == 0
        || availability.protocol_max < availability.protocol_min
        || availability.total_slots == 0
        || availability.total_slots > MAX_WORKER_SLOTS
        || availability.available_slots > availability.total_slots
        || availability.data_classifications.is_empty()
        || availability.capabilities.len() > MAX_FLEET_SET_VALUES
        || availability.egress_profile_ids.len() > MAX_FLEET_SET_VALUES
        || availability.tenant_cache_namespaces.len() > MAX_FLEET_SET_VALUES
        || (availability.worker_status != WorkerStatus::Online && availability.available_slots != 0)
        || !matches!(
            availability.worker_status,
            WorkerStatus::Online | WorkerStatus::Draining | WorkerStatus::Offline
        )
    {
        return Err(FleetSchedulerError::Invalid(
            "invalid worker availability bounds".to_string(),
        ));
    }
    for value in [
        &availability.daemon_version,
        &availability.region,
        &availability.zone,
        &availability.resource_class,
    ] {
        validate_key(value, "worker availability field")?;
    }
    validate_sha256_digest(&availability.image_digest)?;
    for value in availability
        .capabilities
        .iter()
        .chain(&availability.egress_profile_ids)
        .chain(&availability.tenant_cache_namespaces)
    {
        validate_code(value, "worker capability")?;
    }
    Ok(())
}

fn validate_pull(request: &FleetPullRequest) -> Result<(), FleetSchedulerError> {
    let worker = request
        .workload
        .worker_id
        .as_ref()
        .ok_or(FleetSchedulerError::Denied(
            FleetDenialReason::IdentityMismatch,
        ))?;
    let incarnation =
        request
            .workload
            .worker_incarnation_id
            .as_ref()
            .ok_or(FleetSchedulerError::Denied(
                FleetDenialReason::IdentityMismatch,
            ))?;
    validate_workload(&request.workload, worker, incarnation, request.observed_at)?;
    if request.protocol_version == 0
        || request.heartbeat_max_age_seconds == 0
        || request.heartbeat_max_age_seconds > 300
        || request.lease_expires_at <= request.observed_at
        || request.lease_expires_at.saturating_sub(request.observed_at) > 300
    {
        return Err(FleetSchedulerError::Invalid(
            "invalid pull protocol, heartbeat age, or lease expiry".to_string(),
        ));
    }
    Ok(())
}

fn validate_artifact_ack(request: &ArtifactUploadAckRequest) -> Result<(), FleetSchedulerError> {
    validate_report_workload(&request.workload, &request.claim, request.observed_at)?;
    validate_id(request.upload_id.as_str(), "au_", "upload id")?;
    validate_id(
        request.execution_artifact_id.as_str(),
        "ar_",
        "execution artifact id",
    )?;
    validate_key(&request.idempotency_key, "artifact idempotency key")?;
    validate_sha256(&request.artifact_sha256, "artifact sha256")?;
    if request.upload_attempt == 0 || request.part_count == 0 {
        return Err(FleetSchedulerError::Invalid(
            "upload attempt and part count must be positive".to_string(),
        ));
    }
    Ok(())
}

fn validate_side_effect(request: &FleetSideEffectRequest) -> Result<(), FleetSchedulerError> {
    validate_report_workload(&request.workload, &request.claim, request.observed_at)?;
    validate_key(&request.idempotency_key, "side-effect idempotency key")?;
    if !matches!(
        request.checkpoint,
        SecurityCheckpoint::ArtifactAcceptance
            | SecurityCheckpoint::Delivery
            | SecurityCheckpoint::Release
    ) {
        return Err(FleetSchedulerError::Denied(
            FleetDenialReason::SideEffectDenied,
        ));
    }
    if matches!(
        request.action,
        ProtectedAction::SandboxPrepare | ProtectedAction::SandboxExecute
    ) {
        return Err(FleetSchedulerError::Denied(
            FleetDenialReason::SideEffectDenied,
        ));
    }
    Ok(())
}

fn validate_reap(request: &FleetReapRequest) -> Result<(), FleetSchedulerError> {
    if request.observed_at < 0
        || request.heartbeat_stale_before < 0
        || request.lease_expired_before < 0
        || request.heartbeat_stale_before > request.observed_at
        || request.lease_expired_before > request.observed_at
    {
        return Err(FleetSchedulerError::Invalid(
            "invalid reaper time bounds".to_string(),
        ));
    }
    Ok(())
}

fn validate_report_workload(
    workload: &agentd_core::types::AuthenticatedWorkload,
    claim: &TaskLeaseClaim,
    observed_at: i64,
) -> Result<(), FleetSchedulerError> {
    validate_id(claim.execution_task_id.as_str(), "tr_", "execution task id")?;
    validate_id(
        claim.worker_incarnation_id.as_str(),
        "wi_",
        "worker incarnation id",
    )?;
    validate_id(claim.lease_id.as_str(), "ls_", "lease id")?;
    if claim.fencing_token == 0 {
        return Err(FleetSchedulerError::Invalid(
            "lease fencing token must be positive".to_string(),
        ));
    }
    let worker_id = workload
        .worker_id
        .as_ref()
        .ok_or(FleetSchedulerError::Denied(
            FleetDenialReason::IdentityMismatch,
        ))?;
    validate_workload(
        workload,
        worker_id,
        &claim.worker_incarnation_id,
        observed_at,
    )
}

fn validate_workload(
    workload: &agentd_core::types::AuthenticatedWorkload,
    worker_id: &WorkerId,
    incarnation: &WorkerIncarnationId,
    observed_at: i64,
) -> Result<(), FleetSchedulerError> {
    let valid_ids = validate_id(worker_id.as_str(), "wk_", "worker id").is_ok()
        && validate_id(incarnation.as_str(), "wi_", "worker incarnation id").is_ok();
    let expected_spiffe = format!("spiffe://{}/worker/{}", workload.trust_domain, incarnation);
    if workload.role != WorkloadRole::Worker
        || !valid_ids
        || workload.worker_id.as_ref() != Some(worker_id)
        || workload.worker_incarnation_id.as_ref() != Some(incarnation)
        || !valid_trust_domain(&workload.trust_domain)
        || workload.spiffe_uri != expected_spiffe
        || !is_sha256(&workload.certificate_sha256)
        || workload.not_before < 0
        || workload.not_after <= workload.not_before
        || observed_at < workload.not_before
        || observed_at >= workload.not_after
    {
        return Err(FleetSchedulerError::Denied(
            FleetDenialReason::IdentityMismatch,
        ));
    }
    Ok(())
}

fn validate_key(value: &str, field: &str) -> Result<(), FleetSchedulerError> {
    if value != value.trim()
        || value.is_empty()
        || value.len() > 256
        || value.chars().any(char::is_control)
    {
        return Err(FleetSchedulerError::Invalid(format!(
            "{field} must be within 1..=256 bytes"
        )));
    }
    Ok(())
}

fn validate_code(value: &str, field: &str) -> Result<(), FleetSchedulerError> {
    validate_key(value, field)?;
    if !value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b':')
    }) {
        return Err(FleetSchedulerError::Invalid(format!(
            "{field} contains unsupported characters"
        )));
    }
    Ok(())
}

fn validate_sha256(value: &str, field: &str) -> Result<(), FleetSchedulerError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(FleetSchedulerError::Invalid(format!(
            "{field} must be lowercase sha256"
        )));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_trust_domain(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

fn validate_sha256_digest(value: &str) -> Result<(), FleetSchedulerError> {
    let digest = value
        .strip_prefix("sha256:")
        .ok_or_else(|| FleetSchedulerError::Invalid("image digest must use sha256".to_string()))?;
    validate_sha256(digest, "image digest")
}

fn validate_id(value: &str, prefix: &str, field: &str) -> Result<(), FleetSchedulerError> {
    let payload = value
        .strip_prefix(prefix)
        .ok_or_else(|| FleetSchedulerError::Invalid(format!("invalid {field} prefix")))?;
    if payload.len() != 26
        || !payload
            .parse::<ulid::Ulid>()
            .is_ok_and(|parsed| parsed.to_string() == payload)
    {
        return Err(FleetSchedulerError::Invalid(format!("invalid {field}")));
    }
    Ok(())
}

fn retry_delay_seconds(attempt_count: i64) -> i64 {
    let exponent = u32::try_from(attempt_count.saturating_sub(1).clamp(0, 6)).unwrap_or(6);
    5_i64
        .saturating_mul(2_i64.saturating_pow(exponent))
        .min(300)
}

async fn begin_immediate(
    pool: &SqlitePool,
) -> Result<SqliteImmediateTransaction, FleetSchedulerError> {
    SqliteImmediateTransaction::begin(pool)
        .await
        .map_err(storage_error)
}

async fn commit(connection: &mut SqliteImmediateTransaction) -> Result<(), FleetSchedulerError> {
    connection.commit().await.map_err(storage_error)
}

async fn rollback(connection: &mut SqliteImmediateTransaction) -> Result<(), FleetSchedulerError> {
    connection.rollback().await.map_err(storage_error)
}

fn storage_error(error: sqlx::Error) -> FleetSchedulerError {
    FleetSchedulerError::Unavailable(format!("durable fleet scheduler storage: {error}"))
}

fn lease_error(error: agentd_core::ports::TaskLeaseError) -> FleetSchedulerError {
    use agentd_core::ports::TaskLeaseError;
    match error {
        TaskLeaseError::Invalid(message) => FleetSchedulerError::Invalid(message),
        TaskLeaseError::NotFound(message) => FleetSchedulerError::NotFound(message),
        TaskLeaseError::Conflict(message) => FleetSchedulerError::Conflict(message),
        TaskLeaseError::Rejected { .. } => {
            FleetSchedulerError::Denied(FleetDenialReason::StaleFencingToken)
        }
        TaskLeaseError::Unavailable(message) => FleetSchedulerError::Unavailable(message),
    }
}

fn authority_error(error: impl std::fmt::Display) -> FleetSchedulerError {
    FleetSchedulerError::Unavailable(format!("invalid durable authority reference: {error}"))
}

fn authority(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
) -> Result<AuthorityKey, FleetSchedulerError> {
    AuthorityKey::new(row.get::<String, _>(column)).map_err(authority_error)
}

fn sha256(value: &impl Serialize) -> Result<String, FleetSchedulerError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| FleetSchedulerError::Invalid(error.to_string()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn json(value: &impl Serialize) -> Result<String, FleetSchedulerError> {
    serde_json::to_string(value).map_err(|error| FleetSchedulerError::Invalid(error.to_string()))
}

fn parse_json<T: serde::de::DeserializeOwned>(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
) -> Result<T, FleetSchedulerError> {
    serde_json::from_str(&row.get::<String, _>(column))
        .map_err(|error| FleetSchedulerError::Unavailable(error.to_string()))
}

fn token_to_i64(token: FencingToken) -> Result<i64, FleetSchedulerError> {
    u64_to_i64(token.value(), "fencing token")
}

fn fencing_from_i64(value: i64) -> Result<FencingToken, FleetSchedulerError> {
    FencingToken::new(i64_to_u64(value, "fencing token")?)
        .map_err(|error| FleetSchedulerError::Unavailable(error.to_string()))
}

fn u64_to_i64(value: u64, field: &str) -> Result<i64, FleetSchedulerError> {
    i64::try_from(value)
        .map_err(|_| FleetSchedulerError::Invalid(format!("{field} exceeds sqlite integer")))
}

fn i64_to_u64(value: i64, field: &str) -> Result<u64, FleetSchedulerError> {
    u64::try_from(value)
        .map_err(|_| FleetSchedulerError::Unavailable(format!("invalid durable {field}")))
}

fn i64_to_u32(value: i64, field: &str) -> Result<u32, FleetSchedulerError> {
    u32::try_from(value)
        .map_err(|_| FleetSchedulerError::Unavailable(format!("invalid durable {field}")))
}

const fn u32_to_i64(value: u32) -> i64 {
    value as i64
}
