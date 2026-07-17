//! Command-line surface for `agentctl`. P0.1 ships `flow validate`; P0.8 adds
//! `run start` (the standalone Path-B trigger); more subcommands arrive later.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// agentd control CLI.
#[derive(Debug, Parser)]
#[command(name = "agentctl", version)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Final offline import and native-authority cutover operations.
    #[command(subcommand)]
    Cutover(CutoverCmd),
    /// Agent registry and lifecycle operations.
    #[command(subcommand)]
    Agent(AgentCmd),
    /// Workflow (`.dot`) operations.
    #[command(subcommand)]
    Flow(FlowCmd),
    /// Run operations (start a standalone Path-B workflow run).
    #[command(subcommand)]
    Run(RunCmd),
    /// Agent-chat replacement parity operations.
    #[command(subcommand)]
    Parity(ParityCmd),
    /// Native runtime inspection and control.
    #[command(subcommand)]
    Runtime(RuntimeCmd),
    /// Enterprise scale, rollout, compliance, and recovery operations.
    #[command(subcommand)]
    Enterprise(EnterpriseCmd),
}

#[derive(Debug, Subcommand)]
pub enum EnterpriseCmd {
    /// Read the bounded enterprise operational snapshot.
    Status(EnterpriseDaemonArgs),
    /// Explain one durable fleet task and its exact policy block.
    Explain(EnterpriseExplainArgs),
    /// Declare a digest-pinned signed worker image rollout.
    Rollout(EnterpriseMutationFileArgs),
    /// Record one zone's signed-image rollout observation.
    RolloutObserve(EnterpriseMutationFileArgs),
    /// Roll back one declared or active worker image rollout.
    RolloutRollback(EnterpriseMutationFileArgs),
    /// Create or update one zone pool policy.
    ZonePolicy(EnterpriseMutationFileArgs),
    /// Record one capacity observation and scaling recommendation.
    Capacity(EnterpriseMutationFileArgs),
    /// Declare a multi-region artifact replication plan.
    ReplicationPlan(EnterpriseMutationFileArgs),
    /// Acknowledge one immutable artifact replica.
    ReplicaAck(EnterpriseMutationFileArgs),
    /// Register an opaque tenant KMS key/version reference.
    TenantKey(EnterpriseMutationFileArgs),
    /// Transition a tenant key from active to retiring or retiring to retired.
    TenantKeyTransition(EnterpriseMutationFileArgs),
    /// Set a versioned retention policy.
    Retention(EnterpriseMutationFileArgs),
    /// Place an immutable legal hold.
    LegalHold(EnterpriseMutationFileArgs),
    /// Release an active legal hold.
    LegalHoldRelease(EnterpriseLegalHoldReleaseArgs),
    /// Record an immutable disaster-recovery checkpoint.
    DrCheckpoint(EnterpriseMutationFileArgs),
    /// Record a disaster-recovery drill result.
    DrDrill(EnterpriseMutationFileArgs),
    /// Register a pinned factory load model.
    LoadModel(EnterpriseMutationFileArgs),
    /// Record one service-level and error-budget measurement.
    ServiceLevel(EnterpriseMutationFileArgs),
    /// Enroll a stable worker, current incarnation, and public mTLS identity binding.
    WorkerEnroll(EnterpriseMutationFileArgs),
    /// Revoke one worker certificate fingerprint without deleting its history.
    WorkerIdentityRevoke(EnterpriseMutationFileArgs),
}

#[derive(Debug, Clone, Args)]
pub struct EnterpriseDaemonArgs {
    /// Enterprise agentd base URL. HTTPS is required except explicit loopback development.
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    pub daemon_url: String,
    /// Operator bearer token. Falls back to `AGENTD_API_TOKEN`.
    #[arg(long)]
    pub api_token: Option<String>,
    /// PEM CA used to verify a private enterprise operator endpoint.
    #[arg(long)]
    pub server_ca_pem: Option<PathBuf>,
    /// Permit plain HTTP only for an explicit loopback development daemon.
    #[arg(long)]
    pub allow_loopback_http: bool,
}

#[derive(Debug, Args)]
pub struct EnterpriseExplainArgs {
    pub execution_task_id: String,
    #[command(flatten)]
    pub daemon: EnterpriseDaemonArgs,
}

#[derive(Debug, Args)]
pub struct EnterpriseMutationFileArgs {
    /// JSON file containing the exact typed enterprise resource.
    #[arg(long)]
    pub file: PathBuf,
    #[command(flatten)]
    pub daemon: EnterpriseDaemonArgs,
}

#[derive(Debug, Args)]
pub struct EnterpriseLegalHoldReleaseArgs {
    pub legal_hold_id: String,
    #[arg(long)]
    pub released_at: i64,
    #[command(flatten)]
    pub daemon: EnterpriseDaemonArgs,
}

#[derive(Debug, Subcommand)]
pub enum CutoverCmd {
    /// Freeze a canonical offline source digest and create a cutover run.
    Plan(CutoverPlanArgs),
    /// Import every supported agent-chat state surface.
    Import(CutoverSourceStepArgs),
    /// Compare normalized legacy and native decisions.
    Shadow(CutoverSourceStepArgs),
    /// Confirm all imported and source work is terminal.
    Drain(CutoverSourceStepArgs),
    /// Persist acknowledged project cursor handoffs from a JSON file.
    Handoff(CutoverHandoffArgs),
    /// Transfer production authority to agentd.
    Activate(CutoverActivateArgs),
    /// Record final legacy retirement after operator authorization.
    Retire(CutoverMutationArgs),
    /// Inspect one durable cutover run.
    Inspect(CutoverInspectArgs),
    /// Terminate a cutover and preserve rollback evidence.
    Rollback(CutoverRollbackArgs),
    /// Run bounded structured control-plane diagnostics.
    Doctor(CutoverDoctorArgs),
    /// Create a consistent `SQLite` backup and signed-by-digest manifest.
    Backup(CutoverBackupArgs),
    /// Restore a verified backup while the daemon is offline.
    Restore(CutoverRestoreArgs),
    /// Install a native-only service manifest and record its digest.
    ServiceInstall(CutoverServiceInstallArgs),
}

#[derive(Debug, Args)]
pub struct CutoverPlanArgs {
    #[arg(long)]
    pub agent_chat: PathBuf,
    #[arg(long)]
    pub db_path: PathBuf,
    #[arg(long, default_value_t = 86_400)]
    pub rollback_window_seconds: u64,
}

#[derive(Debug, Args)]
pub struct CutoverSourceStepArgs {
    pub cutover_id: String,
    #[arg(long)]
    pub agent_chat: PathBuf,
    #[arg(long)]
    pub db_path: PathBuf,
    #[arg(long)]
    pub idempotency_key: String,
}

#[derive(Debug, Args)]
pub struct CutoverHandoffArgs {
    pub cutover_id: String,
    #[arg(long)]
    pub handoffs_file: PathBuf,
    #[arg(long)]
    pub db_path: PathBuf,
    #[arg(long)]
    pub idempotency_key: String,
}

#[derive(Debug, Args)]
pub struct CutoverActivateArgs {
    pub cutover_id: String,
    #[arg(long)]
    pub agent_chat: PathBuf,
    #[arg(long)]
    pub db_path: PathBuf,
    #[arg(long, default_value_t = 0)]
    pub required_project_handoffs: u32,
    #[arg(long)]
    pub idempotency_key: String,
}

#[derive(Debug, Args)]
pub struct CutoverMutationArgs {
    pub cutover_id: String,
    #[arg(long)]
    pub db_path: PathBuf,
    #[arg(long)]
    pub idempotency_key: String,
}

#[derive(Debug, Args)]
pub struct CutoverInspectArgs {
    pub cutover_id: String,
    #[arg(long)]
    pub db_path: PathBuf,
}

#[derive(Debug, Args)]
pub struct CutoverRollbackArgs {
    pub cutover_id: String,
    #[arg(long)]
    pub db_path: PathBuf,
    #[arg(long)]
    pub idempotency_key: String,
    #[arg(long)]
    pub reason: String,
}

#[derive(Debug, Args)]
pub struct CutoverDoctorArgs {
    #[arg(long)]
    pub db_path: PathBuf,
}

#[derive(Debug, Args)]
pub struct CutoverBackupArgs {
    pub cutover_id: String,
    #[arg(long)]
    pub db_path: PathBuf,
    #[arg(long)]
    pub output: PathBuf,
}

#[derive(Debug, Args)]
pub struct CutoverRestoreArgs {
    #[arg(long)]
    pub db_path: PathBuf,
    #[arg(long)]
    pub backup: PathBuf,
    #[arg(long)]
    pub manifest: PathBuf,
    #[arg(long, default_value = "127.0.0.1:8787")]
    pub daemon_address: String,
}

#[derive(Debug, Args)]
pub struct CutoverServiceInstallArgs {
    pub cutover_id: String,
    #[arg(long)]
    pub db_path: PathBuf,
    #[arg(long, value_enum)]
    pub model: CutoverServiceModel,
    #[arg(long)]
    pub target: PathBuf,
    #[arg(long)]
    pub agentd_bin: PathBuf,
    #[arg(long, default_value_t = 8787)]
    pub port: u16,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CutoverServiceModel {
    Local,
    Team,
    Fleet,
}

#[derive(Debug, Subcommand)]
pub enum RuntimeCmd {
    /// Inspect one logical runtime session.
    Inspect(RuntimeInspectArgs),
    /// Wait for semantic runtime progress.
    Wait(RuntimeWaitArgs),
    /// Send text to the current native attempt.
    SendText(RuntimeSendTextArgs),
    /// Interrupt the current native attempt with Ctrl-C.
    Interrupt(RuntimeInterruptArgs),
    /// Gracefully stop the current native attempt.
    Shutdown(RuntimeShutdownArgs),
}

#[derive(Debug, Args)]
pub struct RuntimeDaemonArgs {
    /// The agentd daemon base URL.
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    pub daemon_url: String,
    /// Operator bearer token. Falls back to `AGENTD_API_TOKEN`.
    #[arg(long)]
    pub api_token: Option<String>,
}

#[derive(Debug, Args)]
pub struct RuntimeInspectArgs {
    pub session_id: String,
    #[command(flatten)]
    pub daemon: RuntimeDaemonArgs,
}

#[derive(Debug, Args)]
pub struct RuntimeWaitArgs {
    pub session_id: String,
    #[arg(long)]
    pub attempt_id: String,
    #[arg(long, default_value_t = 0)]
    pub after_event_index: u64,
    #[arg(long, default_value_t = 25_000)]
    pub timeout_ms: u64,
    #[command(flatten)]
    pub daemon: RuntimeDaemonArgs,
}

#[derive(Debug, Args)]
pub struct RuntimeSendTextArgs {
    pub session_id: String,
    #[arg(long)]
    pub attempt_id: String,
    #[arg(long)]
    pub admission_file: PathBuf,
    #[arg(long)]
    pub idempotency_key: String,
    #[arg(long)]
    pub text: String,
    #[arg(long)]
    pub submit: bool,
    #[command(flatten)]
    pub daemon: RuntimeDaemonArgs,
}

#[derive(Debug, Args)]
pub struct RuntimeInterruptArgs {
    pub session_id: String,
    #[arg(long)]
    pub attempt_id: String,
    #[arg(long)]
    pub admission_file: PathBuf,
    #[arg(long)]
    pub idempotency_key: String,
    #[command(flatten)]
    pub daemon: RuntimeDaemonArgs,
}

#[derive(Debug, Args)]
pub struct RuntimeShutdownArgs {
    pub session_id: String,
    #[arg(long)]
    pub attempt_id: String,
    #[arg(long)]
    pub admission_file: PathBuf,
    #[arg(long)]
    pub idempotency_key: String,
    #[arg(long, value_enum, default_value_t = RuntimeShutdownReason::Cancelled)]
    pub reason: RuntimeShutdownReason,
    #[arg(long, default_value_t = 5_000)]
    pub graceful_timeout_ms: u64,
    #[arg(long, default_value_t = 2_000)]
    pub interrupt_timeout_ms: u64,
    #[command(flatten)]
    pub daemon: RuntimeDaemonArgs,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum RuntimeShutdownReason {
    Completed,
    Failed,
    Cancelled,
    IdleTimeout,
    RuntimeGone,
    WorkerLost,
}

impl RuntimeShutdownReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::IdleTimeout => "idle_timeout",
            Self::RuntimeGone => "runtime_gone",
            Self::WorkerLost => "worker_lost",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum RunCmd {
    /// Start a workflow run from a local issue/spec (standalone, Path B).
    Start(RunStartArgs),
}

#[derive(Debug, Subcommand)]
pub enum AgentCmd {
    /// List registered agents.
    Ls(AgentListArgs),
    /// Inspect one registered agent.
    Inspect(AgentInspectArgs),
    /// Inspect one registered agent's launch environment profile.
    LaunchEnv(AgentLaunchEnvArgs),
    /// Start one registered agent.
    Start(AgentStartArgs),
    /// Stop one registered agent runtime and mark it offline.
    Down(AgentDownArgs),
    /// Rebind one registered agent from its stored runtime target.
    Rebind(AgentRebindArgs),
    /// Record a runtime observation for one agent.
    Runtime(AgentRuntimeArgs),
    /// Register or update an agent.
    Register(AgentRegisterArgs),
    /// Send an agent heartbeat.
    Heartbeat(AgentHeartbeatArgs),
    /// Mark an agent offline.
    Offline(AgentOfflineArgs),
}

#[derive(Debug, Args)]
pub struct AgentListArgs {
    /// The agentd daemon base URL.
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    pub daemon_url: String,
    /// Operator bearer token. Falls back to `AGENTD_API_TOKEN`.
    #[arg(long)]
    pub api_token: Option<String>,
}

#[derive(Debug, Args)]
pub struct AgentInspectArgs {
    /// Agent name.
    pub name: String,
    /// The agentd daemon base URL.
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    pub daemon_url: String,
    /// Operator bearer token. Falls back to `AGENTD_API_TOKEN`.
    #[arg(long)]
    pub api_token: Option<String>,
}

#[derive(Debug, Args)]
pub struct AgentLaunchEnvArgs {
    /// Agent name.
    pub name: String,
    /// The agentd daemon base URL.
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    pub daemon_url: String,
    /// Operator bearer token. Falls back to `AGENTD_API_TOKEN`.
    #[arg(long)]
    pub api_token: Option<String>,
}

#[derive(Debug, Args)]
pub struct AgentStartArgs {
    /// Agent name.
    pub name: String,
    /// The agentd daemon base URL.
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    pub daemon_url: String,
    /// Operator bearer token. Falls back to `AGENTD_API_TOKEN`.
    #[arg(long)]
    pub api_token: Option<String>,
}

#[derive(Debug, Args)]
pub struct AgentDownArgs {
    /// Agent name.
    pub name: String,
    /// The agentd daemon base URL.
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    pub daemon_url: String,
    /// Operator bearer token. Falls back to `AGENTD_API_TOKEN`.
    #[arg(long)]
    pub api_token: Option<String>,
}

#[derive(Debug, Args)]
pub struct AgentRebindArgs {
    /// Agent name.
    pub name: String,
    /// The agentd daemon base URL.
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    pub daemon_url: String,
    /// Operator bearer token. Falls back to `AGENTD_API_TOKEN`.
    #[arg(long)]
    pub api_token: Option<String>,
}

#[derive(Debug, Args)]
pub struct AgentRuntimeArgs {
    /// Agent name.
    pub name: String,
    /// Mark the agent as blocked.
    #[arg(long)]
    pub blocked: bool,
    #[arg(long)]
    pub reason: Option<String>,
    #[arg(long)]
    pub active_now: Option<bool>,
    #[arg(long)]
    pub active_duration_sec: Option<i64>,
    #[arg(long)]
    pub idle_duration_sec: Option<i64>,
    #[arg(long)]
    pub last_runtime_activity_sec: Option<i64>,
    #[arg(long)]
    pub workspace_path: Option<String>,
    #[arg(long)]
    pub mcp_present: Option<bool>,
    /// The agentd daemon base URL.
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    pub daemon_url: String,
    /// Per-agent token. Falls back to `AGENTD_AGENT_TOKEN` or `AGENTCHAT_AGENT_TOKEN`.
    #[arg(long)]
    pub agent_token: Option<String>,
}

#[derive(Debug, Args)]
pub struct AgentRegisterArgs {
    /// Agent name.
    pub name: String,
    #[arg(long)]
    pub role: Option<String>,
    #[arg(long)]
    pub capability: Option<String>,
    #[arg(long)]
    pub runtime: Option<String>,
    #[arg(long)]
    pub model: Option<String>,
    #[arg(long)]
    pub native_runtime_ref: Option<String>,
    #[arg(long)]
    pub home_dir: Option<String>,
    #[arg(long)]
    pub workdir: Option<String>,
    #[arg(long)]
    pub state_dir: Option<String>,
    #[arg(long)]
    pub server: Option<String>,
    /// The agentd daemon base URL.
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    pub daemon_url: String,
    /// Per-agent token. Falls back to `AGENTD_AGENT_TOKEN` or `AGENTCHAT_AGENT_TOKEN`.
    #[arg(long)]
    pub agent_token: Option<String>,
}

#[derive(Debug, Args)]
pub struct AgentHeartbeatArgs {
    /// Agent name.
    pub name: String,
    #[arg(long)]
    pub server: Option<String>,
    #[arg(long)]
    pub native_runtime_ref: Option<String>,
    #[arg(long)]
    pub workspace_path: Option<String>,
    /// The agentd daemon base URL.
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    pub daemon_url: String,
    /// Per-agent token. Falls back to `AGENTD_AGENT_TOKEN` or `AGENTCHAT_AGENT_TOKEN`.
    #[arg(long)]
    pub agent_token: Option<String>,
}

#[derive(Debug, Args)]
pub struct AgentOfflineArgs {
    /// Agent name.
    pub name: String,
    #[arg(long)]
    pub reason: Option<String>,
    /// Keep the native runtime reference while marking it offline.
    #[arg(long)]
    pub no_clear_runtime: bool,
    /// The agentd daemon base URL.
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    pub daemon_url: String,
    /// Per-agent token. Falls back to `AGENTD_AGENT_TOKEN` or `AGENTCHAT_AGENT_TOKEN`.
    #[arg(long)]
    pub agent_token: Option<String>,
}

/// Which standalone Path-B workflow to run.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Flow {
    /// `draft.dot` — issue → spec draft.
    Draft,
    /// `execute.dot` — frozen spec → PR.
    Execute,
    /// `spike.dot` — exploratory throwaway (no gate/review/PR).
    Spike,
    /// `docs-only.dot` — a docs change (linear, no review).
    DocsOnly,
    /// `bugfix-rapid.dot` — a fast fix (keeps the gate, skips review).
    BugfixRapid,
    /// `refactor-only.dot` — behavior-preserving (keeps gate + review).
    RefactorOnly,
    /// `bootstrap.dot` — derive a starter spec from an existing codebase.
    Bootstrap,
}

impl Flow {
    /// The workflow file name for this flow.
    #[must_use]
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Draft => "draft.dot",
            Self::Execute => "execute.dot",
            Self::Spike => "spike.dot",
            Self::DocsOnly => "docs-only.dot",
            Self::BugfixRapid => "bugfix-rapid.dot",
            Self::RefactorOnly => "refactor-only.dot",
            Self::Bootstrap => "bootstrap.dot",
        }
    }

    /// The flow's wire name for the `POST /runs` body — the file stem, identical
    /// to the daemon's `flow_to_file` arm (the flow triple's shared string).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Execute => "execute",
            Self::Spike => "spike",
            Self::DocsOnly => "docs-only",
            Self::BugfixRapid => "bugfix-rapid",
            Self::RefactorOnly => "refactor-only",
            Self::Bootstrap => "bootstrap",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Flow;
    use clap::ValueEnum;
    use std::path::PathBuf;

    #[test]
    fn cli_flow_variants_map_to_existing_files() {
        let wf = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workflows");
        for flow in Flow::value_variants() {
            let file = flow.file_name();
            assert!(wf.join(file).exists(), "Flow file '{file}' must exist");
            // name() is the file stem — the wire string shared with the daemon's
            // flow_to_file arm; this keeps the flow triple from drifting.
            assert_eq!(
                format!("{}.dot", flow.name()),
                file,
                "name() + .dot must equal file_name()"
            );
        }
    }
}

#[derive(Debug, Args)]
pub struct RunStartArgs {
    /// Which standalone workflow to run (draft, execute, spike, docs-only,
    /// bugfix-rapid, refactor-only, bootstrap).
    #[arg(long, value_enum)]
    pub flow: Flow,
    /// The run id — an issue id (draft / bugfix-rapid / docs-only / spike), a
    /// frozen-spec id (execute / refactor-only), or a repo label (bootstrap).
    pub id: String,
    /// Optional run-context file for the run.
    #[arg(long)]
    pub context_file: Option<PathBuf>,
    /// Directory holding the workflow `.dot` files.
    #[arg(long, default_value = "workflows")]
    pub workflows_dir: PathBuf,
    /// The agentd daemon base URL for a live run (ignored by `--dry-run`).
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    pub daemon_url: String,
    /// Validate + print the resolved plan without launching a live run.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Subcommand)]
pub enum ParityCmd {
    /// Audit the agent-chat replacement parity map.
    Audit(ParityAuditArgs),
    /// Plan or execute an agent-chat agents.json import.
    ImportAgents(ParityAgentImportArgs),
    /// Compare agent-chat agents.json against an agentd `SQLite` database.
    ShadowAgents(ParityAgentShadowArgs),
    /// Plan or execute an agent-chat messages/groups/cursors import.
    ImportMessages(ParityMessageImportArgs),
    /// Compare agent-chat messages.json against an agentd `SQLite` database.
    ShadowMessages(ParityMessageShadowArgs),
    /// Plan or execute an agent-chat `tasks/task_graphs` import.
    ImportTasks(ParityTaskImportArgs),
    /// Compare agent-chat `tasks/task_graphs` JSON against an agentd `SQLite` database.
    ShadowTasks(ParityTaskShadowArgs),
}

#[derive(Debug, Args)]
pub struct ParityAuditArgs {
    /// Path to the checked-out agent-chat repository.
    #[arg(long)]
    pub agent_chat: PathBuf,
    /// Path to the parity map Markdown file.
    #[arg(long, default_value = "docs/parity/agent-chat-capability-map.md")]
    pub map: PathBuf,
}

#[derive(Debug, Args)]
pub struct ParityAgentImportArgs {
    /// Path to the checked-out agent-chat repository.
    #[arg(long)]
    pub agent_chat: PathBuf,
    /// Path to the target agentd `SQLite` database.
    #[arg(long)]
    pub db_path: PathBuf,
    /// Execute the import. Without this flag the command is a dry-run plan.
    #[arg(long)]
    pub execute: bool,
}

#[derive(Debug, Args)]
pub struct ParityAgentShadowArgs {
    /// Path to the checked-out agent-chat repository.
    #[arg(long)]
    pub agent_chat: PathBuf,
    /// Path to the target agentd `SQLite` database.
    #[arg(long)]
    pub db_path: PathBuf,
}

#[derive(Debug, Args)]
pub struct ParityMessageImportArgs {
    /// Path to the checked-out agent-chat repository.
    #[arg(long)]
    pub agent_chat: PathBuf,
    /// Path to the target agentd `SQLite` database.
    #[arg(long)]
    pub db_path: PathBuf,
    /// Execute the import. Without this flag the command is a dry-run plan.
    #[arg(long)]
    pub execute: bool,
}

#[derive(Debug, Args)]
pub struct ParityMessageShadowArgs {
    /// Path to the checked-out agent-chat repository.
    #[arg(long)]
    pub agent_chat: PathBuf,
    /// Path to the target agentd `SQLite` database.
    #[arg(long)]
    pub db_path: PathBuf,
}

#[derive(Debug, Args)]
pub struct ParityTaskImportArgs {
    /// Path to the checked-out agent-chat repository.
    #[arg(long)]
    pub agent_chat: PathBuf,
    /// Path to the target agentd `SQLite` database.
    #[arg(long)]
    pub db_path: PathBuf,
    /// Execute the import. Without this flag the command is a dry-run plan.
    #[arg(long)]
    pub execute: bool,
}

#[derive(Debug, Args)]
pub struct ParityTaskShadowArgs {
    /// Path to the checked-out agent-chat repository.
    #[arg(long)]
    pub agent_chat: PathBuf,
    /// Path to the target agentd `SQLite` database.
    #[arg(long)]
    pub db_path: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum FlowCmd {
    /// Validate a workflow `.dot` file against the §2.7 rules.
    Validate(ValidateArgs),
}

#[derive(Debug, Args)]
pub struct ValidateArgs {
    /// Path to the `.dot` workflow file.
    pub path: PathBuf,
}
