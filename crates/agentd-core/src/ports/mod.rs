//! Trait *ports* — the only way agentd-core reaches the outside world (design
//! §4). The engine and handlers depend on these traits; concrete I/O impls
//! (native runtime, sqlite store, mempal client) live in other crates, and
//! in-memory fakes live in [`crate::test_support`].

pub mod agent_allocator;
pub mod backend;
pub mod certification;
pub mod clock;
pub mod command_runner;
pub mod cutover;
pub mod enterprise_scale;
pub mod execution_evidence;
pub mod fleet_scheduler;
pub mod matrix_gateway;
pub mod mempal;
pub mod native_runtime;
pub mod principal;
pub mod project_authority;
pub mod revocation;
pub mod security;
pub mod store;
pub mod task_lease;
pub mod worktree_allocator;

pub use agent_allocator::{
    AgentAllocation, AgentAllocationRequest, AgentAllocationStatus, AgentAllocator,
    DirectAgentAllocator,
};
pub use backend::AgentBackend;
pub use certification::{
    CertificationError, CertificationPort, CertificationRequest, CertificationResultPayload,
    CertificationStatePort, CertificationStateTransition, CertificationVerdict,
    DeliveryCertificationState, EXECUTION_EVIDENCE_ENVELOPE_SCHEMA_VERSION,
    EvidenceArtifactSubject, EvidenceEnvelopeStorePort, EvidenceSignerRole, EvidenceSigningPort,
    EvidenceVerificationPort, ExecutionEvidenceEnvelopePayload, ForgeAdmission,
    ForgeAdmissionRequest, ForgeOperation, ImmutableEvidenceRef,
    OPENFAB_CERTIFICATION_SCHEMA_VERSION, SignedCertificationResult,
    SignedExecutionEvidenceEnvelope, SigningKeyTrustPort, SkillHubPort, SkillInstallAdmission,
    SkillInstallRequest, SkillPackageEvidenceRef, SkillPackageTrustPayload,
    SkillPackageTrustRecord, SkillPackageTrustStatus, TrustedSigningKey, canonical_json,
    canonical_sha256,
};
pub use clock::Clock;
pub use command_runner::{CommandError, CommandOutput, CommandRunner, RunOpts};
pub use cutover::{
    BackupManifest, CursorHandoff, CutoverError, CutoverLedgerPort, CutoverPlan, CutoverRun,
    CutoverSourceManifest, CutoverState, CutoverStepReceipt, CutoverSurface, CutoverTransition,
    LegacyIdMapping, ServiceInstallation, ServiceModel, ShadowDecision,
};
pub use enterprise_scale::{
    ArtifactReplicaAcknowledgement, ArtifactReplicationPlan, AutoscalingRecommendation,
    CapacityObservation, ControlPlaneHeartbeatRequest, ControlPlaneLeadershipLease,
    ControlPlaneLeadershipRenewal, ControlPlaneLeadershipRequest, ControlPlaneMember,
    ControlPlaneMemberStatus, DisasterRecoveryCheckpoint, DisasterRecoveryDrill,
    DisasterRecoveryDrillStatus, EnterpriseMutationFence, EnterpriseOperationalSnapshot,
    EnterpriseScaleError, EnterpriseScalePort, EnterpriseZoneStatus, LegalHold,
    LoadModelRegistration, ReplicaStatus, RetentionDecision, RetentionDisposition, RetentionPolicy,
    ServiceLevelMeasurement, ServiceLevelStatus, TenantKeyStatus, TenantKeyTransition,
    TenantKeyVersion, WorkerImageRollout, WorkerImageRolloutStatus, WorkerImageZoneObservation,
    ZonePoolPolicy,
};
pub use execution_evidence::{
    ArtifactCursor, ArtifactIndexPort, ArtifactListRequest, ArtifactPage, AuditActorKind,
    AuditPage, AuditReadRequest, CertificationReferenceAppend, CertificationReferenceKind,
    CertificationReferencePort, CertificationReferenceRecord, ExecutionArtifactKind,
    ExecutionArtifactPublish, ExecutionArtifactRecord, ExecutionAuditAppend, ExecutionAuditPort,
    ExecutionAuditRecord, ExecutionEvidenceError, ExecutionEvidenceLinks,
    ExecutionEvidenceValidationError, ExecutionSnapshotLink, PageLimit, UsageLedgerPort,
    UsageMeasurement, UsageMetric, UsagePage, UsageReadRequest, UsageRecord, UsageTotal,
    UsageTotals, WorkerArtifactReport, WorkerUsageReport,
};
pub use fleet_scheduler::{
    ArtifactUploadAck, ArtifactUploadAckRequest, FleetAssignment, FleetCancelRequest,
    FleetCompletionReport, FleetDenialReason, FleetExplain, FleetFailureReport,
    FleetHeartbeatRequest, FleetOutboxEvent, FleetPullRequest, FleetQueueStatus, FleetReapRequest,
    FleetReapSummary, FleetRenewRequest, FleetSchedulerError, FleetSchedulerPort,
    FleetSideEffectAdmission, FleetSideEffectRequest, FleetSubmitRequest, FleetTaskRecord,
    FleetTaskRequirements, WorkerAvailability,
};
pub use matrix_gateway::{
    MatrixAttachmentRef, MatrixCommandClass, MatrixCommandDisposition, MatrixCommandReceipt,
    MatrixExecutionSummaryStatus, MatrixGatewayCommandRequest, MatrixGatewayCutoverRequest,
    MatrixGatewayDeliveryPort, MatrixGatewayDenialReason, MatrixGatewayError,
    MatrixGatewayIdentityPort, MatrixGatewayMappingKind, MatrixGatewayMode,
    MatrixGatewayOutboxRecord, MatrixGatewayPort, MatrixGatewayProjectConfig,
    MatrixGatewayRollbackManifest, MatrixGatewayStateMapping, MatrixGatewayStateMappingRequest,
    MatrixGatewaySummaryPublish, MatrixTransportProvenance, NormalizedMatrixCommand,
    RobrixApprovalView, RobrixArtifactView, RobrixCommandView, RobrixEvidenceView,
    RobrixProjectView, RobrixRunView, RobrixRuntimeView, RobrixTaskView,
};
pub use mempal::{DrawerHit, MempalClient};
pub use native_runtime::{
    DurableRuntimeAttempt, DurableRuntimeSession, InteractiveSandboxPort, NativeRuntimeError,
    RuntimeArchivePort, RuntimeBackend, RuntimeCommand, RuntimeDimensions, RuntimeEvent,
    RuntimeEventKind, RuntimeEventPayload, RuntimeEventPort, RuntimeHandle, RuntimeInputAck,
    RuntimeKey, RuntimeKeyInput, RuntimeLaunchRequest, RuntimeLedgerPort, RuntimeProvider,
    RuntimeRecoveryDisposition, RuntimeRecoveryRecord, RuntimeRecoveryRequest,
    RuntimeResizeRequest, RuntimeSandboxCommandRequest, RuntimeSandboxRef,
    RuntimeSessionRegistration, RuntimeShutdownMethod, RuntimeShutdownReport,
    RuntimeShutdownRequest, RuntimeSnapshot, RuntimeTerminalReason, RuntimeTextInput,
    RuntimeTranscriptRef, RuntimeView, RuntimeWaitRequest,
};
pub use principal::EnterprisePrincipalPort;
pub use project_authority::{
    ProjectAuthorityAvailability, ProjectAuthorityError, ProjectAuthorityHealth,
    ProjectAuthorityMode, ProjectAuthorityPort, ProjectSnapshotResolveRequest,
};
pub use revocation::PolicyRevocationPort;
pub use security::{
    AttemptCapabilityPort, ContentRedactionPort, ExecutionSandboxPort, PlacementAdmissionPort,
    SecretBrokerPort, SecurityError, TenantAuthorizationPort, WorkloadIdentityPort,
};
pub use store::{RunStatus, Store};
pub use task_lease::{
    TaskLeaseCloseRequest, TaskLeaseDispatchRequest, TaskLeaseError, TaskLeasePort,
    TaskLeaseRejectionReason, TaskLeaseRenewRequest,
};
pub use worktree_allocator::WorktreeAllocator;
