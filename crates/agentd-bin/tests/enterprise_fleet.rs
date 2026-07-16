use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use agentd_bin::fleet::{
    EnterpriseFleetProviders, FleetProviderKind, build_enterprise_fleet_service,
};
use agentd_core::ports::{
    ArtifactUploadAck, ArtifactUploadAckRequest, FleetAssignment, FleetCancelRequest,
    FleetCompletionReport, FleetExplain, FleetFailureReport, FleetHeartbeatRequest,
    FleetOutboxEvent, FleetPullRequest, FleetReapRequest, FleetReapSummary, FleetRenewRequest,
    FleetSchedulerError, FleetSchedulerPort, FleetSideEffectAdmission, FleetSideEffectRequest,
    FleetSubmitRequest, FleetTaskRecord, SecurityError, WorkerAvailability, WorkloadIdentityPort,
};
use agentd_core::test_support::FixedClock;
use agentd_core::types::{
    AuthenticatedWorkload, DataClassification, FleetOutboxId, TaskLeaseGrant, TaskRunId, WorkerId,
    WorkerIncarnationId, WorkerStatus, WorkloadIdentityRequest, WorkloadRole,
};

#[derive(Debug)]
struct IdentityPort {
    deny: bool,
    observations: Mutex<Vec<i64>>,
}

#[async_trait::async_trait]
impl WorkloadIdentityPort for IdentityPort {
    async fn authenticate_workload(
        &self,
        request: &WorkloadIdentityRequest,
    ) -> Result<AuthenticatedWorkload, SecurityError> {
        self.observations
            .lock()
            .expect("observations")
            .push(request.observed_at);
        if self.deny {
            return Err(SecurityError::Denied(
                agentd_core::types::SecurityDenialReason::IdentityUntrusted,
            ));
        }
        Ok(workload())
    }
}

#[derive(Debug, Default)]
struct RecordingScheduler {
    heartbeats: Mutex<Vec<FleetHeartbeatRequest>>,
}

#[async_trait::async_trait]
impl FleetSchedulerPort for RecordingScheduler {
    async fn submit_task(
        &self,
        _request: &FleetSubmitRequest,
    ) -> Result<FleetTaskRecord, FleetSchedulerError> {
        unavailable()
    }

    async fn heartbeat(
        &self,
        request: &FleetHeartbeatRequest,
    ) -> Result<WorkerAvailability, FleetSchedulerError> {
        self.heartbeats
            .lock()
            .expect("heartbeats")
            .push(request.clone());
        Ok(request.availability.clone())
    }

    async fn pull(
        &self,
        _request: &FleetPullRequest,
    ) -> Result<Option<FleetAssignment>, FleetSchedulerError> {
        unavailable()
    }

    async fn renew(
        &self,
        _request: &FleetRenewRequest,
    ) -> Result<TaskLeaseGrant, FleetSchedulerError> {
        unavailable()
    }

    async fn complete(
        &self,
        _request: &FleetCompletionReport,
    ) -> Result<FleetTaskRecord, FleetSchedulerError> {
        unavailable()
    }

    async fn fail(
        &self,
        _request: &FleetFailureReport,
    ) -> Result<FleetTaskRecord, FleetSchedulerError> {
        unavailable()
    }

    async fn cancel(
        &self,
        _request: &FleetCancelRequest,
    ) -> Result<FleetTaskRecord, FleetSchedulerError> {
        unavailable()
    }

    async fn acknowledge_artifact_upload(
        &self,
        _request: &ArtifactUploadAckRequest,
    ) -> Result<ArtifactUploadAck, FleetSchedulerError> {
        unavailable()
    }

    async fn admit_side_effect(
        &self,
        _request: &FleetSideEffectRequest,
    ) -> Result<FleetSideEffectAdmission, FleetSchedulerError> {
        unavailable()
    }

    async fn reap(
        &self,
        _request: &FleetReapRequest,
    ) -> Result<FleetReapSummary, FleetSchedulerError> {
        unavailable()
    }

    async fn outbox_after(
        &self,
        _after: Option<&FleetOutboxId>,
        _limit: u32,
    ) -> Result<Vec<FleetOutboxEvent>, FleetSchedulerError> {
        unavailable()
    }

    async fn explain(
        &self,
        _execution_task_id: &TaskRunId,
    ) -> Result<Option<FleetExplain>, FleetSchedulerError> {
        Ok(None)
    }
}

fn unavailable<T>() -> Result<T, FleetSchedulerError> {
    Err(FleetSchedulerError::Unavailable("unused".to_string()))
}

fn worker_id() -> WorkerId {
    WorkerId::from_string("wk_01ARZ3NDEKTSV4RRFFQ69G5FAV")
}

fn incarnation_id() -> WorkerIncarnationId {
    WorkerIncarnationId::from_string("wi_01ARZ3NDEKTSV4RRFFQ69G5FAW")
}

fn workload() -> AuthenticatedWorkload {
    AuthenticatedWorkload {
        spiffe_uri: format!("spiffe://workers.example/worker/{}", incarnation_id()),
        role: WorkloadRole::Worker,
        trust_domain: "workers.example".to_string(),
        certificate_sha256: "a".repeat(64),
        not_before: 100,
        not_after: 300,
        worker_id: Some(worker_id()),
        worker_incarnation_id: Some(incarnation_id()),
    }
}

fn heartbeat() -> FleetHeartbeatRequest {
    FleetHeartbeatRequest {
        workload: AuthenticatedWorkload {
            spiffe_uri: "spiffe://attacker/worker/forged".to_string(),
            role: WorkloadRole::Worker,
            trust_domain: "attacker".to_string(),
            certificate_sha256: "b".repeat(64),
            not_before: 0,
            not_after: i64::MAX,
            worker_id: Some(WorkerId::new()),
            worker_incarnation_id: Some(WorkerIncarnationId::new()),
        },
        availability: WorkerAvailability {
            worker_id: worker_id(),
            worker_incarnation_id: incarnation_id(),
            heartbeat_sequence: 1,
            worker_status: WorkerStatus::Online,
            daemon_version: "0.0.0-ad-e2".to_string(),
            protocol_min: 1,
            protocol_max: 1,
            region: "eu-west-1".to_string(),
            zone: "zone-a".to_string(),
            resource_class: "standard".to_string(),
            capabilities: BTreeSet::from(["runtime:codex".to_string()]),
            total_slots: 1,
            available_slots: 1,
            data_classifications: BTreeSet::from([DataClassification::Restricted]),
            image_digest: format!("sha256:{}", "c".repeat(64)),
            image_signature_verified: true,
            dedicated_pool: true,
            egress_profile_ids: BTreeSet::from(["deny-all".to_string()]),
            tenant_cache_namespaces: BTreeSet::from(["org/project".to_string()]),
        },
        observed_at: -1,
    }
}

fn identity_request() -> WorkloadIdentityRequest {
    WorkloadIdentityRequest {
        peer_certificates_der: vec![vec![1, 2, 3]],
        observed_at: -1,
    }
}

#[tokio::test]
async fn fleet_service_uses_authenticated_workload_and_trusted_time() {
    let identity = Arc::new(IdentityPort {
        deny: false,
        observations: Mutex::new(Vec::new()),
    });
    let scheduler = Arc::new(RecordingScheduler::default());
    let service = build_enterprise_fleet_service(EnterpriseFleetProviders::new(
        Arc::clone(&identity) as Arc<dyn WorkloadIdentityPort>,
        Arc::clone(&scheduler) as Arc<dyn FleetSchedulerPort>,
        Arc::new(FixedClock::new(150)),
    ))
    .expect("service");

    service
        .heartbeat(identity_request(), heartbeat())
        .await
        .expect("heartbeat");
    assert_eq!(
        identity
            .observations
            .lock()
            .expect("observations")
            .as_slice(),
        &[150]
    );
    let recorded = scheduler.heartbeats.lock().expect("heartbeats");
    assert_eq!(recorded[0].observed_at, 150);
    assert_eq!(recorded[0].workload, workload());
}

#[tokio::test]
async fn fleet_service_rejects_missing_or_denied_identity_before_scheduler_mutation() {
    for missing in FleetProviderKind::ALL {
        let identity = Arc::new(IdentityPort {
            deny: false,
            observations: Mutex::new(Vec::new()),
        });
        let scheduler = Arc::new(RecordingScheduler::default());
        let error = build_enterprise_fleet_service(
            EnterpriseFleetProviders::new(
                identity as Arc<dyn WorkloadIdentityPort>,
                scheduler as Arc<dyn FleetSchedulerPort>,
                Arc::new(FixedClock::new(150)),
            )
            .without(missing),
        )
        .expect_err("missing provider");
        assert!(error.to_string().contains(missing.as_str()));
    }

    let identity = Arc::new(IdentityPort {
        deny: true,
        observations: Mutex::new(Vec::new()),
    });
    let scheduler = Arc::new(RecordingScheduler::default());
    let service = build_enterprise_fleet_service(EnterpriseFleetProviders::new(
        identity as Arc<dyn WorkloadIdentityPort>,
        Arc::clone(&scheduler) as Arc<dyn FleetSchedulerPort>,
        Arc::new(FixedClock::new(150)),
    ))
    .expect("service");
    let error = service
        .heartbeat(identity_request(), heartbeat())
        .await
        .expect_err("identity denial");
    assert_eq!(error.code, "worker_identity_denied");
    assert!(scheduler.heartbeats.lock().expect("heartbeats").is_empty());
}
