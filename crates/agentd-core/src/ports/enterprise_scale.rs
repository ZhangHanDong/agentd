//! Enterprise control-plane scale, region, compliance, and recovery contracts.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::{
    ArtifactReplicationId, ControlPlaneInstanceId, DisasterRecoveryCheckpointId,
    DisasterRecoveryDrillId, ExecutionArtifactId, LegalHoldId, LoadModelId, TenantKeyId,
    WorkerImageRolloutId, ZonePoolId,
};

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value),+
                }
            }
        }

        impl TryFrom<&str> for $name {
            type Error = &'static str;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    _ => Err(concat!("invalid ", stringify!($name))),
                }
            }
        }
    };
}

string_enum!(ControlPlaneMemberStatus {
    Ready => "ready",
    Draining => "draining",
    Offline => "offline",
});

string_enum!(WorkerImageRolloutStatus {
    Declared => "declared",
    Progressing => "progressing",
    Healthy => "healthy",
    Degraded => "degraded",
    RolledBack => "rolled_back",
});

string_enum!(ReplicaStatus {
    Pending => "pending",
    Available => "available",
    Failed => "failed",
});

string_enum!(TenantKeyStatus {
    Active => "active",
    Retiring => "retiring",
    Retired => "retired",
});

string_enum!(RetentionDisposition {
    Retain => "retain",
    LegalHold => "legal_hold",
    ReplicationPending => "replication_pending",
    DeleteEligible => "delete_eligible",
});

string_enum!(DisasterRecoveryDrillStatus {
    Passed => "passed",
    Failed => "failed",
});

string_enum!(ServiceLevelStatus {
    WithinObjective => "within_objective",
    BudgetWarning => "budget_warning",
    Breached => "breached",
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPlaneMember {
    pub instance_id: ControlPlaneInstanceId,
    pub heartbeat_sequence: u64,
    pub region: String,
    pub zone: String,
    pub daemon_version: String,
    pub endpoint_sha256: String,
    pub status: ControlPlaneMemberStatus,
    pub started_at: i64,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPlaneHeartbeatRequest {
    pub idempotency_key: String,
    pub member: ControlPlaneMember,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPlaneLeadershipRequest {
    pub instance_id: ControlPlaneInstanceId,
    pub idempotency_key: String,
    pub observed_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPlaneLeadershipRenewal {
    pub instance_id: ControlPlaneInstanceId,
    pub idempotency_key: String,
    pub term: u64,
    pub fencing_token: u64,
    pub observed_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPlaneLeadershipLease {
    pub instance_id: ControlPlaneInstanceId,
    pub term: u64,
    pub fencing_token: u64,
    pub acquired_at: i64,
    pub renewed_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerImageRollout {
    pub rollout_id: WorkerImageRolloutId,
    pub image_digest: String,
    pub signature_bundle_sha256: String,
    pub policy_sha256: String,
    pub required_zones: BTreeSet<String>,
    pub status: WorkerImageRolloutStatus,
    pub declared_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerImageZoneObservation {
    pub rollout_id: WorkerImageRolloutId,
    pub zone: String,
    pub observed_image_digest: String,
    pub signature_verified: bool,
    pub ready_workers: u32,
    pub desired_workers: u32,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZonePoolPolicy {
    pub pool_id: ZonePoolId,
    pub region: String,
    pub zone: String,
    pub resource_class: String,
    pub trust_domain: String,
    pub rollout_id: WorkerImageRolloutId,
    pub minimum_replicas: u32,
    pub maximum_replicas: u32,
    pub target_queue_per_slot: u32,
    pub scale_down_cooldown_seconds: u32,
    pub enabled: bool,
    pub policy_sha256: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapacityObservation {
    pub pool_id: ZonePoolId,
    pub queue_depth: u64,
    pub running_tasks: u64,
    pub ready_replicas: u32,
    pub total_slots: u32,
    pub available_slots: u32,
    pub last_scale_at: Option<i64>,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoscalingRecommendation {
    pub pool_id: ZonePoolId,
    pub current_replicas: u32,
    pub desired_replicas: u32,
    pub queue_depth: u64,
    pub reason_code: String,
    pub recommendation_sha256: String,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactReplicationPlan {
    pub replication_id: ArtifactReplicationId,
    pub execution_artifact_id: ExecutionArtifactId,
    pub tenant_scope_sha256: String,
    pub artifact_sha256: String,
    pub source_region: String,
    pub required_regions: BTreeSet<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactReplicaAcknowledgement {
    pub replication_id: ArtifactReplicationId,
    pub region: String,
    pub artifact_sha256: String,
    pub object_ref_sha256: String,
    pub tenant_key_id: TenantKeyId,
    pub status: ReplicaStatus,
    pub acknowledged_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantKeyVersion {
    pub tenant_key_id: TenantKeyId,
    pub tenant_scope_sha256: String,
    pub region: String,
    pub kms_key_ref_sha256: String,
    pub key_version_ref_sha256: String,
    pub status: TenantKeyStatus,
    pub activated_at: i64,
    pub retired_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionPolicy {
    pub tenant_scope_sha256: String,
    pub policy_version_sha256: String,
    pub artifact_retention_seconds: u64,
    pub transcript_retention_seconds: u64,
    pub audit_retention_seconds: u64,
    pub minimum_replica_regions: u32,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegalHold {
    pub legal_hold_id: LegalHoldId,
    pub tenant_scope_sha256: String,
    pub subject_kind: String,
    pub subject_sha256: String,
    pub reason_sha256: String,
    pub active: bool,
    pub placed_at: i64,
    pub released_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionDecision {
    pub tenant_scope_sha256: String,
    pub subject_kind: String,
    pub subject_sha256: String,
    pub disposition: RetentionDisposition,
    pub policy_version_sha256: String,
    pub delete_after: i64,
    pub active_legal_holds: u32,
    pub available_replica_regions: u32,
    pub required_replica_regions: u32,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisasterRecoveryCheckpoint {
    pub checkpoint_id: DisasterRecoveryCheckpointId,
    pub region: String,
    pub database_sha256: String,
    pub artifact_index_sha256: String,
    pub audit_head_sha256: String,
    pub matrix_cursor_sha256: String,
    pub certification_head_sha256: String,
    pub maximum_rpo_seconds: u32,
    pub maximum_rto_seconds: u32,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisasterRecoveryDrill {
    pub drill_id: DisasterRecoveryDrillId,
    pub checkpoint_id: DisasterRecoveryCheckpointId,
    pub recovery_region: String,
    pub measured_rpo_seconds: u32,
    pub measured_rto_seconds: u32,
    pub lease_fencing_verified: bool,
    pub accepted_state_verified: bool,
    pub status: DisasterRecoveryDrillStatus,
    pub evidence_sha256: String,
    pub completed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadModelRegistration {
    pub load_model_id: LoadModelId,
    pub version: String,
    pub content_sha256: String,
    pub dimensions: BTreeSet<String>,
    pub test_window_seconds: u32,
    pub tenant_count: u32,
    pub project_count: u32,
    pub room_count: u32,
    pub matrix_events_per_second: u32,
    pub maximum_queue_depth: u64,
    pub noisy_neighbor_ratio_basis_points: u32,
    pub registered_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceLevelMeasurement {
    pub idempotency_key: String,
    pub tenant_scope_sha256: String,
    pub metric: String,
    pub target_units: u64,
    pub observed_units: u64,
    pub error_budget_units: u64,
    pub consumed_budget_units: u64,
    pub window_started_at: i64,
    pub window_ends_at: i64,
    pub measured_at: i64,
}

impl ServiceLevelMeasurement {
    #[must_use]
    pub const fn status(&self) -> ServiceLevelStatus {
        if self.observed_units > self.target_units
            || self.consumed_budget_units > self.error_budget_units
        {
            ServiceLevelStatus::Breached
        } else if self.error_budget_units > 0
            && self.consumed_budget_units.saturating_mul(100)
                >= self.error_budget_units.saturating_mul(80)
        {
            ServiceLevelStatus::BudgetWarning
        } else {
            ServiceLevelStatus::WithinObjective
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnterpriseZoneStatus {
    pub pool_id: ZonePoolId,
    pub region: String,
    pub zone: String,
    pub ready_workers: u32,
    pub available_slots: u32,
    pub queued_tasks: u64,
    pub last_desired_replicas: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnterpriseOperationalSnapshot {
    pub observed_at: i64,
    pub leadership: Option<ControlPlaneLeadershipLease>,
    pub members: Vec<ControlPlaneMember>,
    pub zones: Vec<EnterpriseZoneStatus>,
    pub queued_tasks: u64,
    pub acquired_tasks: u64,
    pub dead_letter_tasks: u64,
    pub active_rollouts: u64,
    pub degraded_rollouts: u64,
    pub pending_replica_regions: u64,
    pub active_legal_holds: u64,
    pub service_level_warnings: u64,
    pub service_level_breaches: u64,
    pub latest_dr_checkpoint: Option<DisasterRecoveryCheckpoint>,
    pub load_model: Option<LoadModelRegistration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EnterpriseScaleError {
    #[error("invalid enterprise scale request: {0}")]
    Invalid(String),
    #[error("enterprise scale resource not found: {0}")]
    NotFound(String),
    #[error("enterprise scale conflict: {0}")]
    Conflict(String),
    #[error("enterprise scale request denied: {0}")]
    Denied(String),
    #[error("enterprise scale control plane unavailable: {0}")]
    Unavailable(String),
}

#[async_trait::async_trait]
pub trait EnterpriseScalePort: Send + Sync {
    async fn heartbeat_control_plane(
        &self,
        request: &ControlPlaneHeartbeatRequest,
    ) -> Result<ControlPlaneMember, EnterpriseScaleError>;

    async fn acquire_leadership(
        &self,
        request: &ControlPlaneLeadershipRequest,
    ) -> Result<ControlPlaneLeadershipLease, EnterpriseScaleError>;

    async fn renew_leadership(
        &self,
        request: &ControlPlaneLeadershipRenewal,
    ) -> Result<ControlPlaneLeadershipLease, EnterpriseScaleError>;

    async fn declare_worker_image_rollout(
        &self,
        rollout: &WorkerImageRollout,
    ) -> Result<WorkerImageRollout, EnterpriseScaleError>;

    async fn observe_worker_image_zone(
        &self,
        observation: &WorkerImageZoneObservation,
    ) -> Result<WorkerImageRollout, EnterpriseScaleError>;

    async fn upsert_zone_pool(
        &self,
        policy: &ZonePoolPolicy,
    ) -> Result<ZonePoolPolicy, EnterpriseScaleError>;

    async fn recommend_capacity(
        &self,
        observation: &CapacityObservation,
    ) -> Result<AutoscalingRecommendation, EnterpriseScaleError>;

    async fn create_replication_plan(
        &self,
        plan: &ArtifactReplicationPlan,
    ) -> Result<ArtifactReplicationPlan, EnterpriseScaleError>;

    async fn acknowledge_artifact_replica(
        &self,
        acknowledgement: &ArtifactReplicaAcknowledgement,
    ) -> Result<ArtifactReplicaAcknowledgement, EnterpriseScaleError>;

    async fn register_tenant_key(
        &self,
        key: &TenantKeyVersion,
    ) -> Result<TenantKeyVersion, EnterpriseScaleError>;

    async fn set_retention_policy(
        &self,
        policy: &RetentionPolicy,
    ) -> Result<RetentionPolicy, EnterpriseScaleError>;

    async fn place_legal_hold(&self, hold: &LegalHold) -> Result<LegalHold, EnterpriseScaleError>;

    async fn release_legal_hold(
        &self,
        legal_hold_id: &LegalHoldId,
        released_at: i64,
    ) -> Result<LegalHold, EnterpriseScaleError>;

    async fn decide_retention(
        &self,
        tenant_scope_sha256: &str,
        subject_kind: &str,
        subject_sha256: &str,
        created_at: i64,
        observed_at: i64,
    ) -> Result<RetentionDecision, EnterpriseScaleError>;

    async fn record_dr_checkpoint(
        &self,
        checkpoint: &DisasterRecoveryCheckpoint,
    ) -> Result<DisasterRecoveryCheckpoint, EnterpriseScaleError>;

    async fn record_dr_drill(
        &self,
        drill: &DisasterRecoveryDrill,
    ) -> Result<DisasterRecoveryDrill, EnterpriseScaleError>;

    async fn register_load_model(
        &self,
        model: &LoadModelRegistration,
    ) -> Result<LoadModelRegistration, EnterpriseScaleError>;

    async fn record_service_level(
        &self,
        measurement: &ServiceLevelMeasurement,
    ) -> Result<ServiceLevelMeasurement, EnterpriseScaleError>;

    async fn operational_snapshot(
        &self,
        observed_at: i64,
    ) -> Result<EnterpriseOperationalSnapshot, EnterpriseScaleError>;
}
