//! Authenticated enterprise worker-fleet service composition.

use std::fmt;
use std::sync::Arc;

use agentd_core::ports::{
    ArtifactUploadAck, ArtifactUploadAckRequest, Clock, FleetAssignment, FleetCancelRequest,
    FleetCompletionReport, FleetDenialReason, FleetExplain, FleetFailureReport,
    FleetHeartbeatRequest, FleetOutboxEvent, FleetPullRequest, FleetReapRequest, FleetReapSummary,
    FleetRenewRequest, FleetSchedulerError, FleetSchedulerPort, FleetSideEffectAdmission,
    FleetSideEffectRequest, FleetSubmitRequest, FleetTaskRecord, SecurityError, WorkerAvailability,
    WorkloadIdentityPort,
};
use agentd_core::types::{
    AuthenticatedWorkload, FleetOutboxId, TaskLeaseGrant, TaskRunId, WorkloadIdentityRequest,
    WorkloadRole,
};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FleetProviderKind {
    WorkloadIdentity,
    Scheduler,
    TrustedClock,
}

impl FleetProviderKind {
    pub const ALL: [Self; 3] = [Self::WorkloadIdentity, Self::Scheduler, Self::TrustedClock];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorkloadIdentity => "workload_identity",
            Self::Scheduler => "fleet_scheduler",
            Self::TrustedClock => "trusted_clock",
        }
    }
}

impl fmt::Display for FleetProviderKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FleetStartupError {
    #[error("enterprise fleet startup missing closed provider: {0}")]
    MissingProvider(FleetProviderKind),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetServiceError {
    pub code: &'static str,
}

impl fmt::Display for FleetServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code)
    }
}

impl std::error::Error for FleetServiceError {}

#[derive(Default)]
pub struct EnterpriseFleetProviders {
    workload_identity: Option<Arc<dyn WorkloadIdentityPort>>,
    scheduler: Option<Arc<dyn FleetSchedulerPort>>,
    trusted_clock: Option<Arc<dyn Clock>>,
}

impl fmt::Debug for EnterpriseFleetProviders {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EnterpriseFleetProviders")
            .field(
                "configured",
                &FleetProviderKind::ALL
                    .into_iter()
                    .filter(|kind| self.has(*kind))
                    .map(FleetProviderKind::as_str)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl EnterpriseFleetProviders {
    #[must_use]
    pub fn new(
        workload_identity: Arc<dyn WorkloadIdentityPort>,
        scheduler: Arc<dyn FleetSchedulerPort>,
        trusted_clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            workload_identity: Some(workload_identity),
            scheduler: Some(scheduler),
            trusted_clock: Some(trusted_clock),
        }
    }

    #[must_use]
    pub fn without(mut self, provider: FleetProviderKind) -> Self {
        match provider {
            FleetProviderKind::WorkloadIdentity => self.workload_identity = None,
            FleetProviderKind::Scheduler => self.scheduler = None,
            FleetProviderKind::TrustedClock => self.trusted_clock = None,
        }
        self
    }

    fn has(&self, provider: FleetProviderKind) -> bool {
        match provider {
            FleetProviderKind::WorkloadIdentity => self.workload_identity.is_some(),
            FleetProviderKind::Scheduler => self.scheduler.is_some(),
            FleetProviderKind::TrustedClock => self.trusted_clock.is_some(),
        }
    }
}

pub fn build_enterprise_fleet_service(
    mut providers: EnterpriseFleetProviders,
) -> Result<EnterpriseFleetService, FleetStartupError> {
    if let Some(missing) = FleetProviderKind::ALL
        .into_iter()
        .find(|provider| !providers.has(*provider))
    {
        return Err(FleetStartupError::MissingProvider(missing));
    }
    Ok(EnterpriseFleetService {
        workload_identity: take(&mut providers.workload_identity),
        scheduler: take(&mut providers.scheduler),
        trusted_clock: take(&mut providers.trusted_clock),
    })
}

fn take<T: ?Sized>(provider: &mut Option<Arc<T>>) -> Arc<T> {
    provider
        .take()
        .expect("provider presence checked before fleet composition")
}

pub struct EnterpriseFleetService {
    workload_identity: Arc<dyn WorkloadIdentityPort>,
    scheduler: Arc<dyn FleetSchedulerPort>,
    trusted_clock: Arc<dyn Clock>,
}

impl fmt::Debug for EnterpriseFleetService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EnterpriseFleetService")
            .field("providers", &"[CONFIGURED]")
            .finish()
    }
}

impl EnterpriseFleetService {
    pub async fn submit_task(
        &self,
        mut request: FleetSubmitRequest,
    ) -> Result<FleetTaskRecord, FleetServiceError> {
        request.submitted_at = self.now()?;
        self.scheduler
            .submit_task(&request)
            .await
            .map_err(map_scheduler_error)
    }

    pub async fn heartbeat(
        &self,
        identity: WorkloadIdentityRequest,
        mut request: FleetHeartbeatRequest,
    ) -> Result<WorkerAvailability, FleetServiceError> {
        let (workload, observed_at) = self.authenticate(identity).await?;
        request.workload = workload;
        request.observed_at = observed_at;
        self.scheduler
            .heartbeat(&request)
            .await
            .map_err(map_scheduler_error)
    }

    pub async fn pull(
        &self,
        identity: WorkloadIdentityRequest,
        mut request: FleetPullRequest,
    ) -> Result<Option<FleetAssignment>, FleetServiceError> {
        let (workload, observed_at) = self.authenticate(identity).await?;
        request.workload = workload;
        request.observed_at = observed_at;
        self.scheduler
            .pull(&request)
            .await
            .map_err(map_scheduler_error)
    }

    pub async fn renew(
        &self,
        identity: WorkloadIdentityRequest,
        mut request: FleetRenewRequest,
    ) -> Result<TaskLeaseGrant, FleetServiceError> {
        let (workload, observed_at) = self.authenticate(identity).await?;
        request.workload = workload;
        request.observed_at = observed_at;
        self.scheduler
            .renew(&request)
            .await
            .map_err(map_scheduler_error)
    }

    pub async fn complete(
        &self,
        identity: WorkloadIdentityRequest,
        mut report: FleetCompletionReport,
    ) -> Result<FleetTaskRecord, FleetServiceError> {
        let (workload, observed_at) = self.authenticate(identity).await?;
        report.workload = workload;
        report.observed_at = observed_at;
        self.scheduler
            .complete(&report)
            .await
            .map_err(map_scheduler_error)
    }

    pub async fn fail(
        &self,
        identity: WorkloadIdentityRequest,
        mut report: FleetFailureReport,
    ) -> Result<FleetTaskRecord, FleetServiceError> {
        let (workload, observed_at) = self.authenticate(identity).await?;
        report.workload = workload;
        report.observed_at = observed_at;
        self.scheduler
            .fail(&report)
            .await
            .map_err(map_scheduler_error)
    }

    pub async fn cancel(
        &self,
        identity: WorkloadIdentityRequest,
        mut request: FleetCancelRequest,
    ) -> Result<FleetTaskRecord, FleetServiceError> {
        let (workload, observed_at) = self.authenticate(identity).await?;
        request.workload = workload;
        request.observed_at = observed_at;
        self.scheduler
            .cancel(&request)
            .await
            .map_err(map_scheduler_error)
    }

    pub async fn acknowledge_artifact_upload(
        &self,
        identity: WorkloadIdentityRequest,
        mut request: ArtifactUploadAckRequest,
    ) -> Result<ArtifactUploadAck, FleetServiceError> {
        let (workload, observed_at) = self.authenticate(identity).await?;
        request.workload = workload;
        request.observed_at = observed_at;
        self.scheduler
            .acknowledge_artifact_upload(&request)
            .await
            .map_err(map_scheduler_error)
    }

    pub async fn admit_side_effect(
        &self,
        identity: WorkloadIdentityRequest,
        mut request: FleetSideEffectRequest,
    ) -> Result<FleetSideEffectAdmission, FleetServiceError> {
        let (workload, observed_at) = self.authenticate(identity).await?;
        request.workload = workload;
        request.observed_at = observed_at;
        self.scheduler
            .admit_side_effect(&request)
            .await
            .map_err(map_scheduler_error)
    }

    pub async fn reap(
        &self,
        mut request: FleetReapRequest,
    ) -> Result<FleetReapSummary, FleetServiceError> {
        request.observed_at = self.now()?;
        self.scheduler
            .reap(&request)
            .await
            .map_err(map_scheduler_error)
    }

    pub async fn outbox_after(
        &self,
        after: Option<&FleetOutboxId>,
        limit: u32,
    ) -> Result<Vec<FleetOutboxEvent>, FleetServiceError> {
        self.scheduler
            .outbox_after(after, limit)
            .await
            .map_err(map_scheduler_error)
    }

    pub async fn explain(
        &self,
        execution_task_id: &TaskRunId,
    ) -> Result<Option<FleetExplain>, FleetServiceError> {
        self.scheduler
            .explain(execution_task_id)
            .await
            .map_err(map_scheduler_error)
    }

    async fn authenticate(
        &self,
        mut request: WorkloadIdentityRequest,
    ) -> Result<(AuthenticatedWorkload, i64), FleetServiceError> {
        let observed_at = self.now()?;
        request.observed_at = observed_at;
        let workload = self
            .workload_identity
            .authenticate_workload(&request)
            .await
            .map_err(map_identity_error)?;
        if workload.role != WorkloadRole::Worker
            || workload.worker_id.is_none()
            || workload.worker_incarnation_id.is_none()
            || observed_at < workload.not_before
            || observed_at >= workload.not_after
        {
            return Err(FleetServiceError {
                code: "worker_identity_mismatch",
            });
        }
        Ok((workload, observed_at))
    }

    fn now(&self) -> Result<i64, FleetServiceError> {
        let observed_at = self.trusted_clock.now_unix();
        if observed_at < 0 {
            return Err(FleetServiceError {
                code: "trusted_clock_unavailable",
            });
        }
        Ok(observed_at)
    }
}

fn map_identity_error(error: SecurityError) -> FleetServiceError {
    let code = match error {
        SecurityError::Denied(_) => "worker_identity_denied",
        SecurityError::Invalid(_) => "worker_identity_invalid",
        SecurityError::Unavailable(_) => "worker_identity_unavailable",
    };
    FleetServiceError { code }
}

fn map_scheduler_error(error: FleetSchedulerError) -> FleetServiceError {
    let code = match error {
        FleetSchedulerError::Denied(reason) => denial_code(reason),
        FleetSchedulerError::Invalid(_) => "fleet_request_invalid",
        FleetSchedulerError::NotFound(_) => "fleet_resource_not_found",
        FleetSchedulerError::Conflict(_) => "fleet_state_conflict",
        FleetSchedulerError::Unavailable(_) => "fleet_scheduler_unavailable",
    };
    FleetServiceError { code }
}

const fn denial_code(reason: FleetDenialReason) -> &'static str {
    match reason {
        FleetDenialReason::IdentityMismatch => "identity_mismatch",
        FleetDenialReason::WorkerNotCurrent => "worker_not_current",
        FleetDenialReason::WorkerNotOnline => "worker_not_online",
        FleetDenialReason::WorkerDraining => "worker_draining",
        FleetDenialReason::HeartbeatStale => "heartbeat_stale",
        FleetDenialReason::HeartbeatSequenceRegressed => "heartbeat_sequence_regressed",
        FleetDenialReason::ProtocolUnsupported => "protocol_unsupported",
        FleetDenialReason::CapacityUnavailable => "capacity_unavailable",
        FleetDenialReason::QuotaExceeded => "quota_exceeded",
        FleetDenialReason::SnapshotExpired => "snapshot_expired",
        FleetDenialReason::RevocationEpochStale => "revocation_epoch_stale",
        FleetDenialReason::PlacementDenied => "placement_denied",
        FleetDenialReason::StaleFencingToken => "stale_fencing_token",
        FleetDenialReason::TaskTerminal => "task_terminal",
        FleetDenialReason::DuplicateMismatch => "duplicate_mismatch",
        FleetDenialReason::ArtifactUploadRejected => "artifact_upload_rejected",
        FleetDenialReason::SideEffectDenied => "side_effect_denied",
    }
}
