//! Node handlers. A `Handler` is what runs when the engine reaches a node; it
//! returns a [`HandlerStep`] — `Done` (synchronous: `conditional`/`tool`) or
//! `Park` (external work: `wait.human`/`fan_out`/`fan_in`/`codergen`, Task 8).
//! The engine (Task 9) drives them via the [`HandlerRegistry`].

pub mod codergen;
pub mod conditional;
pub mod fan_in;
pub mod fan_out;
pub mod registry;
pub mod tool;
pub mod wait_human;

pub use codergen::CodergenHandler;
pub use conditional::ConditionalHandler;
pub use fan_in::FanInHandler;
pub use fan_out::FanOutHandler;
pub use registry::HandlerRegistry;
pub use tool::ToolHandler;
pub use wait_human::WaitHumanHandler;

use crate::CoreError;
use crate::engine::{EngineEvent, HandlerStep};
use crate::graph::{NodeDef, NodeGraph};
use crate::ports::{
    AgentAllocation, AgentAllocator, AgentBackend, Clock, CommandRunner, MempalClient, Store,
    WorktreeAllocator,
};
use crate::types::RunId;

/// A borrow bundle of the five ports. The engine builds one of these per run and
/// threads it into each node's [`HandlerCtx`], keeping the constructor narrow.
#[derive(Clone, Copy)]
pub struct Ports<'a> {
    pub backend: &'a dyn AgentBackend,
    pub runner: &'a dyn CommandRunner,
    pub store: &'a dyn Store,
    pub mempal: &'a dyn MempalClient,
    pub clock: &'a dyn Clock,
    pub agent_allocator: &'a dyn AgentAllocator,
}

impl std::fmt::Debug for Ports<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ports").finish_non_exhaustive()
    }
}

/// Everything a handler needs for one node execution. Borrows the graph, the
/// current node, a read-only view of the run context, and the ports; owns a
/// staged `context_updates` map the handler writes into before returning.
///
/// # Context-update channels (engine invariant)
///
/// There are two ways the run context gets updated, with one reconciliation rule
/// the engine (Task 9) enforces:
/// - **ctx-staged** — [`HandlerCtx::stage`]: updates a handler computes locally
///   before returning, *including before a `Park`*, so the Task 5 checkpoint's
///   `context_snapshot` captures them (e.g. codergen staging its `task_run_id`).
/// - **`Outcome.context_updates`** — updates arriving from outside (an agent's
///   `submit_outcome` via MCP).
///
/// The engine merges ctx-staged into the `RunContext` on **every** step (Done and
/// Park) and additionally merges `Outcome.context_updates` on **Done**. A handler
/// MUST NOT write the same key through both channels.
pub struct HandlerCtx<'a> {
    pub run_id: &'a RunId,
    pub graph: &'a NodeGraph,
    pub node: &'a NodeDef,
    pub context: &'a crate::types::RunContext,
    pub ports: Ports<'a>,
    staged: serde_json::Map<String, serde_json::Value>,
    /// Optional per-task_run allocator (P2 C1' R3a). `None` keeps the mechanism
    /// inert; codergen falls back to `"."`.
    worktree_allocator: Option<&'a dyn WorktreeAllocator>,
}

impl std::fmt::Debug for HandlerCtx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandlerCtx")
            .field("run_id", &self.run_id)
            .field("node", &self.node.id)
            .field("staged_keys", &self.staged.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl<'a> HandlerCtx<'a> {
    #[must_use]
    pub fn new(
        run_id: &'a RunId,
        graph: &'a NodeGraph,
        node: &'a NodeDef,
        context: &'a crate::types::RunContext,
        ports: Ports<'a>,
    ) -> Self {
        Self {
            run_id,
            graph,
            node,
            context,
            ports,
            staged: serde_json::Map::new(),
            worktree_allocator: None,
        }
    }

    /// Thread the optional per-task_run worktree allocator (P2 C1' R3a).
    #[must_use]
    pub fn with_worktree_allocator(mut self, allocator: Option<&'a dyn WorktreeAllocator>) -> Self {
        self.worktree_allocator = allocator;
        self
    }

    /// The optional per-task_run worktree allocator.
    #[must_use]
    pub fn worktree_allocator(&self) -> Option<&dyn WorktreeAllocator> {
        self.worktree_allocator
    }

    /// Stage a context update (the ctx-staged channel — see the type docs).
    pub fn stage(&mut self, key: impl Into<String>, value: serde_json::Value) {
        self.staged.insert(key.into(), value);
    }

    /// The context updates staged so far. The engine merges these on every step.
    #[must_use]
    pub fn staged_updates(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.staged
    }

    /// A node attribute on the current node.
    #[must_use]
    pub fn node_attr(&self, key: &str) -> Option<&str> {
        self.node.attrs.get(key).map(String::as_str)
    }

    /// The current node's outgoing edges, in graph order.
    pub fn outgoing_edges(&self) -> impl Iterator<Item = &crate::graph::EdgeDef> + '_ {
        let id = &self.node.id;
        self.graph.edges.iter().filter(move |e| &e.from == id)
    }
}

/// Lowercase-hex SHA-256 of `data`. Shared by `tool` (artifact pointer) and
/// `fan_out` (the in-memory `context_sha`, Task 8) so both hash identically.
#[must_use]
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Build a `SpawnRequest` for `role` with sensible P0.1 defaults (claude-code
/// CLI, direct launch). Shared by `codergen` and `fan_out`; callers resolve
/// `worktree` from the per-task_run allocation or staged run context, falling
/// back to `"."` when neither exists.
#[must_use]
pub(crate) fn cli_kind_for_role(role: &str) -> crate::types::CliKind {
    if role.starts_with("codex-") {
        crate::types::CliKind::Codex
    } else {
        crate::types::CliKind::ClaudeCode
    }
}

/// Build a `SpawnRequest` for `role` with the default direct launch strategy.
/// Role-name prefixes select the CLI runtime for the p201 Codex-first smoke
/// path: `codex-*` uses Codex, while existing unprefixed and `claude-*` roles
/// remain Claude-compatible.
#[must_use]
pub(crate) fn spawn_request(
    role: &str,
    initial_prompt: Option<String>,
    worktree: &std::path::Path,
) -> crate::types::SpawnRequest {
    use crate::types::{AgentId, LaunchStrategy, SpawnRequest};
    SpawnRequest {
        agent_id: AgentId::parsed(role),
        mxid: None,
        cli: cli_kind_for_role(role),
        worktree: worktree.to_path_buf(),
        initial_prompt,
        env_overrides: std::collections::HashMap::new(),
        launch_strategy: LaunchStrategy::Direct,
    }
}

pub(crate) fn stage_agent_allocation(ctx: &mut HandlerCtx<'_>, allocation: &AgentAllocation) {
    let node_id = ctx.node.id.clone();
    let mut root = ctx
        .staged_updates()
        .get("agentd_scheduler_allocations")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut values = root
        .remove(&node_id)
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    values.push(agent_allocation_json(allocation));
    root.insert(node_id, serde_json::Value::Array(values));
    ctx.stage(
        "agentd_scheduler_allocations",
        serde_json::Value::Object(root),
    );
}

#[must_use]
pub fn agent_allocation_json(allocation: &AgentAllocation) -> serde_json::Value {
    let mut value = serde_json::json!({
        "requestedRole": allocation.requested_role.as_str(),
        "agentId": allocation.agent_id.as_str(),
        "schedulerStatus": allocation.status.as_str(),
    });
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    if let Some(tier) = allocation.tier.as_deref() {
        object.insert(
            "tier".to_string(),
            serde_json::Value::String(tier.to_string()),
        );
    }
    if let Some(reservation_id) = allocation.reservation_id.as_deref() {
        object.insert(
            "schedulerReservationId".to_string(),
            serde_json::Value::String(reservation_id.to_string()),
        );
    }
    if let Some(ticket) = allocation.ticket.as_deref() {
        object.insert(
            "schedulerTicket".to_string(),
            serde_json::Value::String(ticket.to_string()),
        );
    }
    if let Some(name) = allocation.provisioned_name.as_deref() {
        object.insert(
            "provisionedName".to_string(),
            serde_json::Value::String(name.to_string()),
        );
    }
    if !allocation.runtime.is_null() {
        object.insert("runtime".to_string(), allocation.runtime.clone());
    }
    value
}

pub(crate) fn current_node_allocation_agent_ids(
    ctx: &HandlerCtx<'_>,
) -> Vec<crate::types::AgentId> {
    ctx.context
        .get("agentd_scheduler_allocations")
        .and_then(serde_json::Value::as_object)
        .and_then(|allocations| allocations.get(&ctx.node.id))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|allocation| allocation.get("agentId"))
        .filter_map(serde_json::Value::as_str)
        .map(crate::types::AgentId::parsed)
        .collect()
}

pub fn append_agent_allocation_prompt_context(prompt: &mut String, allocation: &AgentAllocation) {
    use std::fmt::Write as _;

    if !prompt.is_empty() && !prompt.ends_with('\n') {
        prompt.push('\n');
    }
    let _ = writeln!(
        prompt,
        "agentd_scheduler_status: {}",
        allocation.status.as_str()
    );
    let _ = writeln!(
        prompt,
        "agentd_scheduler_requested_role: {}",
        allocation.requested_role
    );
    if let Some(tier) = allocation.tier.as_deref() {
        let _ = writeln!(prompt, "agentd_scheduler_tier: {tier}");
    }
    if let Some(reservation_id) = allocation.reservation_id.as_deref() {
        let _ = writeln!(prompt, "agentd_scheduler_reservation_id: {reservation_id}");
    }
    if let Some(ticket) = allocation.ticket.as_deref() {
        let _ = writeln!(prompt, "agentd_scheduler_ticket: {ticket}");
    }
    if let Some(name) = allocation.provisioned_name.as_deref() {
        let _ = writeln!(prompt, "agentd_scheduler_provisioned_name: {name}");
    }
}

/// What runs at a node. Object-safe via `#[async_trait]` so the registry can
/// store `Arc<dyn Handler>`.
#[async_trait::async_trait]
pub trait Handler: Send + Sync {
    /// Execute the node. Returns `Done` (synchronous) or `Park` (external work).
    ///
    /// # Errors
    /// Returns [`CoreError`] on an unrecoverable handler failure (e.g. a node
    /// missing a required attribute).
    async fn run(&self, ctx: &mut HandlerCtx<'_>) -> Result<HandlerStep, CoreError>;

    /// Resume after the parking event arrives. Only park-style handlers override
    /// this; the default rejects an unexpected resume.
    ///
    /// # Errors
    /// Returns [`CoreError::Invariant`] for handlers that never park.
    async fn resume(
        &self,
        _ctx: &mut HandlerCtx<'_>,
        _event: EngineEvent,
    ) -> Result<HandlerStep, CoreError> {
        Err(CoreError::Invariant(
            "handler does not support resume".to_string(),
        ))
    }
}
