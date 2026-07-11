//! Trait *ports* — the only way agentd-core reaches the outside world (design
//! §4). The engine and handlers depend on these traits; concrete I/O impls
//! (tmux backend, sqlite store, mempal client) live in other crates, and
//! in-memory fakes live in [`crate::test_support`].

pub mod agent_allocator;
pub mod backend;
pub mod clock;
pub mod command_runner;
pub mod mempal;
pub mod project_authority;
pub mod store;
pub mod worktree_allocator;

pub use agent_allocator::{
    AgentAllocation, AgentAllocationRequest, AgentAllocationStatus, AgentAllocator,
    DirectAgentAllocator,
};
pub use backend::AgentBackend;
pub use clock::Clock;
pub use command_runner::{CommandError, CommandOutput, CommandRunner, RunOpts};
pub use mempal::{DrawerHit, MempalClient};
pub use project_authority::{
    ProjectAuthorityAvailability, ProjectAuthorityError, ProjectAuthorityHealth,
    ProjectAuthorityMode, ProjectAuthorityPort, ProjectSnapshotResolveRequest,
};
pub use store::{RunStatus, Store};
pub use worktree_allocator::WorktreeAllocator;
