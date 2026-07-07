//! `ProductionRunHost` — the real [`RunHost`] (P0.9): the 5 agent-facing methods
//! over a real `SqliteStore` + a per-call `Engine`. It lives in the daemon crate
//! (the composition root) — NOT in `agentd-surface`, which stays store-free
//! (P0.7 D2). agentd-core stays frozen (D1): `deliver` just constructs `Engine`
//! and calls `deliver_event` (which loads the checkpoint and resumes internally).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agentd_core::CoreError;
use agentd_core::dot::parser;
use agentd_core::engine::{Checkpoint, Engine, EngineEvent, ParkReason, RunProgress};
use agentd_core::graph::NodeGraph;
use agentd_core::handler::{HandlerRegistry, Ports};
use agentd_core::ports::{
    AgentBackend, Clock, CommandRunner, MempalClient, Store, WorktreeAllocator,
};
use agentd_core::types::{NodeId, ReviewRunId, RunId};
use agentd_store::{SqliteStore, event_repo, review_repo, run_repo, task_repo};
use agentd_surface::host::{
    EventRecord, LiveEvent, RunHost, RunSnapshot, RunSummary, TaskAssignment,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;

/// Capacity of the live-event broadcast: a subscriber more than this many events
/// behind lags and is realigned with a snapshot (P1).
const LIVE_BROADCAST_CAPACITY: usize = 256;

/// Per-run lock registry (P2 Foundation A): hands out one `Arc<Mutex>` per run id
/// so the daemon can serialize a run's advancing operations (`deliver`/`start_run`)
/// while different runs proceed concurrently. The std mutex guards only the
/// map's get-or-insert and is never held across an `.await`. No eviction — one
/// entry per distinct run id, bounded by run volume (see p98 Out of Scope).
#[derive(Default)]
struct RunLockRegistry {
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl RunLockRegistry {
    /// The lock for `run_id` — the SAME `Arc` for a given run, a different one
    /// for a different run.
    fn lock_for(&self, run_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = self.locks.lock().expect("run-lock map poisoned");
        map.entry(run_id.to_string()).or_default().clone()
    }
}

/// The daemon's production `RunHost`. Holds the real store + the swappable ports
/// as trait objects (the daemon supplies `TmuxBackend`/`SystemClock`/…; tests
/// supply the in-memory fakes), and re-resolves each run's graph from
/// `runs.workflow_path` under `workflows_dir`.
pub struct ProductionRunHost {
    store: SqliteStore,
    backend: Box<dyn AgentBackend>,
    runner: Box<dyn CommandRunner>,
    mempal: Box<dyn MempalClient>,
    clock: Box<dyn Clock>,
    worktree_allocator: Option<Box<dyn WorktreeAllocator>>,
    registry: HandlerRegistry,
    workflows_dir: PathBuf,
    /// The live-event broadcast (P1): the emit point publishes here for the SSE
    /// tail. Lossy/bounded — `send` never blocks the engine on a slow subscriber.
    live_tx: broadcast::Sender<LiveEvent>,
    /// Per-run delivery serialization (P2 Foundation A): one lock per run id, so
    /// concurrent events for one run can't double-advance it.
    run_locks: RunLockRegistry,
}

impl std::fmt::Debug for ProductionRunHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProductionRunHost")
            .field("workflows_dir", &self.workflows_dir)
            .finish_non_exhaustive()
    }
}

impl ProductionRunHost {
    /// Assemble the production host from the real store + the chosen ports.
    pub fn new(
        store: SqliteStore,
        backend: Box<dyn AgentBackend>,
        runner: Box<dyn CommandRunner>,
        mempal: Box<dyn MempalClient>,
        clock: Box<dyn Clock>,
        workflows_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            store,
            backend,
            runner,
            mempal,
            clock,
            worktree_allocator: None,
            registry: HandlerRegistry::with_builtins(),
            workflows_dir: workflows_dir.into(),
            live_tx: broadcast::channel(LIVE_BROADCAST_CAPACITY).0,
            run_locks: RunLockRegistry::default(),
        }
    }

    /// The underlying store (for the daemon's run-start + recovery paths).
    #[must_use]
    pub fn store(&self) -> &SqliteStore {
        &self.store
    }

    /// Inject the optional per-`task_run` worktree allocator used by codergen.
    #[must_use]
    pub fn with_worktree_allocator(
        mut self,
        allocator: Option<Box<dyn WorktreeAllocator>>,
    ) -> Self {
        self.worktree_allocator = allocator;
        self
    }

    /// Re-read + build the run's graph from `runs.workflow_path`, returning it
    /// with the current content sha. Errors if the run has no recorded path.
    pub async fn resolve_graph(&self, run_id: &RunId) -> Result<(NodeGraph, String), CoreError> {
        let path = run_repo::workflow_path(self.store.pool(), run_id)
            .await?
            .ok_or_else(|| {
                CoreError::Invariant(format!("run '{}' has no workflow_path", run_id.as_str()))
            })?;
        let full = if Path::new(&path).is_absolute() {
            PathBuf::from(&path)
        } else {
            self.workflows_dir.join(&path)
        };
        let src = std::fs::read_to_string(&full)?;
        let sha = sha256_hex(src.as_bytes());
        let ast = parser::parse(&src).map_err(|e| CoreError::DotParse(format!("{e:?}")))?;
        let graph = NodeGraph::from_ast(&ast)?;
        Ok((graph, sha))
    }

    /// Build a per-call `Engine` over the real ports for `graph`/`sha`.
    fn engine<'a>(&'a self, graph: &'a NodeGraph, sha: &str) -> Engine<'a> {
        Engine::new(
            graph,
            &self.registry,
            Ports {
                backend: &*self.backend,
                runner: &*self.runner,
                store: &self.store,
                mempal: &*self.mempal,
                clock: &*self.clock,
            },
            sha.to_string(),
        )
        .with_worktree_allocator(self.worktree_allocator.as_deref())
    }

    /// Start a run: resolve its graph and execute from the start node to the
    /// first park (or completion), emitting the resulting state-change event.
    /// The daemon's run-start path (POST /runs) and the contract tests call this.
    ///
    /// # Errors
    /// [`CoreError`] on a store/handler/engine failure or an unresolved graph.
    pub async fn start_run(&self, run_id: &RunId) -> Result<RunProgress, CoreError> {
        // Serialize per run (P2 Foundation A): every run-advancing op on one run
        // is mutually exclusive, so it can't race a concurrent deliver.
        let lock = self.run_locks.lock_for(run_id.as_str());
        let _guard = lock.lock().await;
        let (graph, sha) = self.resolve_graph(run_id).await?;
        let progress = self.engine(&graph, &sha).execute(run_id).await?;
        self.emit(run_id, &progress).await?;
        Ok(progress)
    }

    /// Emit ONE event row per STATE-CHANGING `RunProgress` (P0.7-deferred emit
    /// point, D6): `Parked`→`run_parked`, `Finished`→`run_finished`,
    /// `Failed`→`run_failed`. `Ignored` emits nothing. The payload is COMPACT
    /// JSON (no newlines — avoids the P0.7 D9 SSE CR/LF hazard).
    async fn emit(&self, run_id: &RunId, progress: &RunProgress) -> Result<(), CoreError> {
        let (kind, payload) = match progress {
            RunProgress::Parked {
                node_id,
                reason: ParkReason::ReviewVerdicts { round, .. },
                ..
            } => (
                "run_parked",
                serde_json::json!({ "node": node_id.as_str(), "round": round }),
            ),
            RunProgress::Parked { node_id, .. } => (
                "run_parked",
                serde_json::json!({ "node": node_id.as_str() }),
            ),
            RunProgress::Finished { .. } => ("run_finished", serde_json::json!({})),
            RunProgress::Failed { reason, .. } => {
                ("run_failed", serde_json::json!({ "reason": reason }))
            }
            RunProgress::Ignored { .. } => return Ok(()),
        };
        let payload = payload.to_string();
        // Dedup consecutive same-node re-parks (P1 re-park-noise gap): a fan-out
        // review re-parks at the SAME node per non-final verdict. If the most
        // recent event is already a `run_parked` for this same node, emit nothing
        // — no durable row AND no broadcast. Distinct-node parks and terminals
        // (kind != "run_parked") always fall through.
        if kind == "run_parked" {
            if let Some((last_kind, last_payload)) =
                event_repo::last(self.store.pool(), run_id).await?
            {
                if last_kind == "run_parked" && last_payload == payload {
                    return Ok(());
                }
            }
        }
        // DUAL-WRITE: persist (durable/audit) then broadcast (the live SSE tail).
        let seq = event_repo::append(self.store.pool(), run_id, kind, &payload).await?;
        // Lossy + non-blocking: an absent/slow subscriber never blocks the engine.
        let _ = self.live_tx.send(LiveEvent {
            run_id: run_id.as_str().to_string(),
            event: EventRecord {
                seq,
                kind: kind.to_string(),
                payload,
            },
        });
        Ok(())
    }

    /// Resolve which run an inbound event belongs to via the store's park
    /// lookups; `None` if no open park matches (a replayed / already-resolved
    /// event — the replay-safe path).
    async fn run_for_event(&self, event: &EngineEvent) -> Result<Option<RunId>, CoreError> {
        let park = match event {
            EngineEvent::HumanAnswered { wait_id, .. } => {
                self.store.lookup_park_by_wait_id(wait_id).await?
            }
            EngineEvent::ReviewVerdictSubmitted { review_run_id, .. } => {
                self.store.lookup_park_by_review_run(review_run_id).await?
            }
            EngineEvent::AgentOutcomeSubmitted { task_run_id, .. } => {
                self.store.lookup_park_by_task_run(task_run_id).await?
            }
        };
        Ok(park.map(|(run_id, _node)| run_id))
    }
}

#[async_trait::async_trait]
impl RunHost for ProductionRunHost {
    fn subscribe_events(&self) -> broadcast::Receiver<LiveEvent> {
        self.live_tx.subscribe()
    }

    async fn list_runs(&self) -> Result<Vec<RunSummary>, CoreError> {
        let rows = run_repo::list_runs(self.store.pool()).await?;
        Ok(rows
            .into_iter()
            .map(|(run_id, status, current_node, started_at)| RunSummary {
                run_id,
                status,
                current_node,
                started_at,
            })
            .collect())
    }

    async fn start_workflow(
        &self,
        flow: &str,
        run_id: &RunId,
        _context: Value,
    ) -> Result<RunProgress, CoreError> {
        let file = flow_to_file(flow)
            .ok_or_else(|| CoreError::Invariant(format!("unknown flow '{flow}'")))?;
        // The initial `context` is accepted but not seeded in the MVP — the
        // shipped workflows use fixed paths, not context vars (real-env gap).
        let src = std::fs::read_to_string(self.workflows_dir.join(file))?;
        let sha = sha256_hex(src.as_bytes());
        run_repo::record_run(self.store.pool(), run_id, file, &sha).await?;
        self.start_run(run_id).await
    }

    async fn deliver(&self, event: EngineEvent) -> Result<RunProgress, CoreError> {
        // Resolve the run from the event; an unmatched event is replay-safe Ignored.
        // (Read-only resolve — safe unlocked; concurrent callers for one run all
        // resolve the same run id while the park is open, then serialize below.)
        let Some(run_id) = self.run_for_event(&event).await? else {
            return Ok(RunProgress::Ignored {
                reason: "no open park matches this event (unknown or already resolved)".to_string(),
            });
        };
        // Serialize per run (P2 Foundation A): the mutation runs INSIDE the lock,
        // so a serialized later caller's `deliver_event` re-resolves the gate and
        // sees the prior insert — exactly one caller advances, the rest re-park.
        let lock = self.run_locks.lock_for(run_id.as_str());
        let _guard = lock.lock().await;
        let (graph, sha) = self.resolve_graph(&run_id).await?;
        let progress = self.engine(&graph, &sha).deliver_event(event).await?;
        self.emit(&run_id, &progress).await?;
        Ok(progress)
    }

    async fn run_snapshot(&self, run_id: &RunId) -> Result<Option<RunSnapshot>, CoreError> {
        let Some((status, current_from_run)) =
            run_repo::read_status(self.store.pool(), run_id).await?
        else {
            return Ok(None);
        };
        let checkpoint: Option<Checkpoint> = self.store.load_checkpoint(run_id).await?;
        let (current_node, completed_nodes, context) = match checkpoint {
            Some(cp) => (
                Some(cp.current_node.as_str().to_string()),
                cp.completed_nodes
                    .iter()
                    .map(|n| n.as_str().to_string())
                    .collect(),
                Value::Object(cp.context_snapshot.0),
            ),
            None => (
                current_from_run,
                Vec::new(),
                Value::Object(serde_json::Map::new()),
            ),
        };
        Ok(Some(RunSnapshot {
            status,
            current_node,
            completed_nodes,
            context,
        }))
    }

    async fn open_task(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
    ) -> Result<Option<TaskAssignment>, CoreError> {
        let found = task_repo::find_open_task_run(self.store.pool(), run_id, node_id).await?;
        let Some((task_run_id, worktree, agent_id)) = found else {
            return Ok(None);
        };
        let checkpoint = self.store.load_checkpoint(run_id).await?;
        Ok(Some(TaskAssignment {
            task_run_id,
            agent_id: agent_id.unwrap_or_default(),
            worktree,
            spec_path: checkpoint_context_string(checkpoint.as_ref(), "spec_path"),
            plan_path: checkpoint_context_string(checkpoint.as_ref(), "plan_path"),
            context_pack: checkpoint_context_string(checkpoint.as_ref(), "context_pack"),
        }))
    }

    async fn review_counts(
        &self,
        review_run_id: &ReviewRunId,
    ) -> Result<(usize, usize), CoreError> {
        let expected = review_repo::review_expected(self.store.pool(), review_run_id)
            .await?
            .unwrap_or(0);
        let got = review_repo::count_verdicts(self.store.pool(), review_run_id).await?;
        Ok((expected, got))
    }

    async fn events_from(
        &self,
        run_id: &RunId,
        after_seq: i64,
    ) -> Result<Vec<EventRecord>, CoreError> {
        let rows = event_repo::read_from(self.store.pool(), run_id, after_seq).await?;
        Ok(rows
            .into_iter()
            .map(|r| EventRecord {
                seq: r.seq,
                kind: r.kind,
                payload: r.payload,
            })
            .collect())
    }
}

fn checkpoint_context_string(checkpoint: Option<&Checkpoint>, key: &str) -> Option<String> {
    checkpoint?
        .context_snapshot
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// Map a wire flow name to its shipped `.dot` file, or `None` for an unknown
/// flow. The single source of truth for the daemon side of the flow triple
/// (clap-derived value / `agentctl::cli::Flow::name` / this arm) — kept as a
/// pure fn so the mapping is unit-testable without starting a run (§7.3 P1.7).
fn flow_to_file(flow: &str) -> Option<&'static str> {
    match flow {
        "draft" => Some("draft.dot"),
        "execute" => Some("execute.dot"),
        "spike" => Some("spike.dot"),
        "docs-only" => Some("docs-only.dot"),
        "bugfix-rapid" => Some("bugfix-rapid.dot"),
        "refactor-only" => Some("refactor-only.dot"),
        "bootstrap" => Some("bootstrap.dot"),
        _ => None,
    }
}

/// Lowercase hex SHA-256 of `data` (the workflow content sha).
fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{ProductionRunHost, RunLockRegistry, flow_to_file};
    use crate::SystemClock;
    use agentd_core::engine::{ParkReason, RunProgress};
    use agentd_core::test_support::{FakeBackend, MempalStub, RecordingCommandRunner};
    use agentd_core::types::{NodeId, ReviewRunId, RunId};
    use agentd_store::{SqliteStore, run_repo};
    use agentd_surface::host::RunHost;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn workflows_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workflows")
    }

    async fn host_for_emit_tests() -> (ProductionRunHost, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SqliteStore::connect(&dir.path().join("agentd.db"))
            .await
            .expect("connect");
        let host = ProductionRunHost::new(
            store,
            Box::new(FakeBackend::new()),
            Box::new(RecordingCommandRunner::new()),
            Box::new(MempalStub::new()),
            Box::new(SystemClock),
            workflows_dir(),
        );
        (host, dir)
    }

    #[test]
    fn run_lock_is_per_run() {
        // The mechanism the per-run serialization rests on: concurrent callers for
        // ONE run must contend on the SAME lock; different runs must not.
        let reg = RunLockRegistry::default();
        let a1 = reg.lock_for("r1");
        let a2 = reg.lock_for("r1");
        let b = reg.lock_for("r2");
        assert!(Arc::ptr_eq(&a1, &a2), "same run id -> the same lock");
        assert!(
            !Arc::ptr_eq(&a1, &b),
            "different run id -> a different lock (cross-run delivery stays concurrent)"
        );
    }

    #[test]
    fn flow_to_file_resolves_every_shipped_flow() {
        // Every shipped flow name maps to a file that EXISTS — catches a
        // start_workflow arm that drifts from the actual file or the CLI name.
        for flow in [
            "draft",
            "execute",
            "spike",
            "docs-only",
            "bugfix-rapid",
            "refactor-only",
            "bootstrap",
        ] {
            let mapped = flow_to_file(flow);
            assert!(
                mapped.is_some(),
                "flow '{flow}' must map to a file, not 'unknown flow'"
            );
            let file = mapped.expect("just asserted Some");
            assert!(
                workflows_dir().join(file).exists(),
                "flow '{flow}' -> '{file}' must exist under workflows/"
            );
        }
        assert!(flow_to_file("bogus").is_none(), "unknown flow -> None");
    }

    #[tokio::test]
    async fn production_runhost_emits_review_reparks_when_round_differs() {
        let (host, _dir) = host_for_emit_tests().await;
        let run = RunId::from_string("round-dedup");
        run_repo::record_run(host.store().pool(), &run, "execute.dot", "sha")
            .await
            .expect("record");

        for round in [1, 2] {
            host.emit(
                &run,
                &RunProgress::Parked {
                    run_id: run.clone(),
                    node_id: NodeId::parsed("review"),
                    reason: ParkReason::ReviewVerdicts {
                        review_run_id: ReviewRunId::from_string(format!("rr{round}")),
                        expected: 3,
                        round,
                    },
                },
            )
            .await
            .expect("emit");
        }

        let events = host.events_from(&run, 0).await.expect("events");
        let payloads: Vec<&str> = events
            .iter()
            .filter(|e| e.kind == "run_parked")
            .map(|e| e.payload.as_str())
            .collect();
        assert_eq!(
            payloads,
            vec![
                r#"{"node":"review","round":1}"#,
                r#"{"node":"review","round":2}"#
            ],
            "different review rounds survive payload-based dedup"
        );
    }
}
