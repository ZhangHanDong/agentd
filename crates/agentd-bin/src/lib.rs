//! `agentd_bin` — the daemon's library surface (P0.9). The composition root: it
//! may depend on the store/runtime/mempal adapters (unlike `agentd-surface`, which
//! stays store-free, P0.7 D2). `main.rs` is the thin entrypoint over this lib so
//! the assembly is integration-testable.

#![warn(clippy::unwrap_used, clippy::panic)]

pub mod agent_mcp_context;
pub mod cli;
pub mod clock;
pub mod command_runner;
pub mod daemon;
pub mod enterprise;
pub mod fleet;
pub mod host;
pub mod matrix_bridge;
pub mod matrix_gateway;
pub mod mempal;
pub mod native_backend;
pub mod openfab;
pub mod openfab_http;
pub mod runtime;
pub mod security;
pub mod stdio_mcp;

pub use cli::{
    AgentdCli, AgentdCommand, CleanupWorktreesArgs, DaemonConfig, EnterpriseDaemonConfig, MatrixBridgeOnceArgs,
    MatrixClientBridgePreflightArgs, MatrixClientBridgeServiceArgs,
};
pub use clock::SystemClock;
pub use host::ProductionRunHost;
pub use mempal::OfflineMempal;
pub use runtime::{
    NativeRuntimeCompositionConfig, NativeRuntimeService, NativeRuntimeStartRequest,
    NativeRuntimeView, compose_native_runtime, provider_command_sha256,
};
pub use security::SecurityRuntimeMode;
