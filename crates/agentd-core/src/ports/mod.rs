//! Trait *ports* — the only way agentd-core reaches the outside world (design
//! §4). The engine and handlers depend on these traits; concrete I/O impls
//! (tmux backend, sqlite store, mempal client) live in other crates, and
//! in-memory fakes live in [`crate::test_support`].

pub mod agent_allocator;
pub mod backend;
pub mod clock;
pub mod command_runner;
pub mod execution_evidence;
pub mod mempal;
pub mod native_runtime;
pub mod project_authority;
pub mod security;
pub mod store;
pub mod task_lease;
pub mod worker_fleet;
pub mod worktree_allocator;

pub use agent_allocator::{
    AgentAllocation, AgentAllocationRequest, AgentAllocationStatus, AgentAllocator,
    DirectAgentAllocator,
};
pub use backend::AgentBackend;
pub use clock::Clock;
pub use command_runner::{CommandError, CommandOutput, CommandRunner, RunOpts};
pub use execution_evidence::{
    ArtifactCursor, ArtifactIndexPort, ArtifactListRequest, ArtifactPage, AuditActorKind,
    AuditPage, AuditReadRequest, CertificationReferenceAppend, CertificationReferenceKind,
    CertificationReferencePort, CertificationReferenceRecord, ExecutionArtifactKind,
    ExecutionArtifactPublish, ExecutionArtifactRecord, ExecutionAuditAppend, ExecutionAuditPort,
    ExecutionAuditRecord, ExecutionEvidenceError, ExecutionEvidenceLinks,
    ExecutionEvidenceValidationError, ExecutionSnapshotLink, PageLimit, UsageLedgerPort,
    UsageMeasurement, UsageMetric, UsagePage, UsageReadRequest, UsageRecord, UsageTotal,
    UsageTotals, WorkerArtifactAcknowledgement, WorkerArtifactReport, WorkerUsageReport,
};
pub use mempal::{DrawerHit, MempalClient};
pub use native_runtime::{
    NativeRuntimeAttemptStart, NativeRuntimeAttemptState, NativeRuntimeControlError,
    NativeRuntimeControlPort, NativeRuntimeSessionValidate, NativeRuntimeSessionView,
};
pub use project_authority::{
    ProjectAuthorityAvailability, ProjectAuthorityError, ProjectAuthorityHealth,
    ProjectAuthorityMode, ProjectAuthorityPort, ProjectSnapshotResolveRequest,
};
pub use security::{
    AuthenticatedWorkload, CapabilityAdmission, ExecutionSecurityScope, MtlsWorkloadVerifier,
    ProtectedAction, ProtectedResource, SecretBrokerPort, SecretLease, SecretMaterial,
    SecurityDenial, TenantAuthorizationPort, WorkloadRole,
};
pub use store::{RunStatus, Store};
pub use task_lease::{
    TaskLeaseCloseRequest, TaskLeaseDispatchRequest, TaskLeaseError, TaskLeasePort,
    TaskLeaseRejectionReason, TaskLeaseRenewRequest,
};
pub use worker_fleet::{
    WorkerFleetDrainRequest, WorkerFleetError, WorkerFleetHeartbeat, WorkerFleetHeartbeatResult,
    WorkerFleetPort, WorkerFleetPullRequest, WorkerFleetRegisterRequest, WorkerFleetRegistration,
};
pub use worktree_allocator::WorktreeAllocator;
