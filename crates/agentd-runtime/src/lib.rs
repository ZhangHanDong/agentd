//! Native PTY/process runtime for agentd.

#![warn(clippy::unwrap_used, clippy::panic)]

pub mod archive;
pub mod provider;
pub mod pty;

pub use archive::ContentAddressedTranscriptStore;
pub use provider::{ProviderCommand, RuntimeProviderAdapter};
pub use pty::{
    NativePtyRuntime, RuntimeChildControl, RuntimeProcessHost, RuntimePtyControl,
    SpawnedRuntimeProcess,
};
