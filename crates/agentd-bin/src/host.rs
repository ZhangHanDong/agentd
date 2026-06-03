//! `ProductionRunHost` — the real [`RunHost`] (P0.9): the 5 agent-facing methods
//! over a real `SqliteStore` + a per-call `Engine`. It lives in the daemon crate
//! (the composition root) — NOT in `agentd-surface`, which stays store-free
//! (P0.7 D2). agentd-core stays frozen (D1): `deliver` just constructs `Engine`
//! and calls `deliver_event` (which loads the checkpoint and resumes internally).

use std::path::{Path, PathBuf};

use agentd_core::CoreError;
use agentd_core::dot::parser;
use agentd_core::engine::{Checkpoint, Engine, EngineEvent, RunProgress};
use agentd_core::graph::NodeGraph;
use agentd_core::handler::{HandlerRegistry, Ports};
use agentd_core::ports::{AgentBackend, Clock, CommandRunner, MempalClient, Store};
use agentd_core::types::{NodeId, ReviewRunId, RunId};
use agentd_store::{SqliteStore, event_repo, review_repo, run_repo, task_repo};
use agentd_surface::host::{EventRecord, LiveEvent, RunHost, RunSnapshot, TaskAssignment};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;

/// Capacity of the live-event broadcast: a subscriber more than this many events
/// behind lags and is realigned with a snapshot (P1).
const LIVE_BROADCAST_CAPACITY: usize = 256;

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
    registry: HandlerRegistry,
    workflows_dir: PathBuf,
    /// The live-event broadcast (P1): the emit point publishes here for the SSE
    /// tail. Lossy/bounded — `send` never blocks the engine on a slow subscriber.
    live_tx: broadcast::Sender<LiveEvent>,
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
            registry: HandlerRegistry::with_builtins(),
            workflows_dir: workflows_dir.into(),
            live_tx: broadcast::channel(LIVE_BROADCAST_CAPACITY).0,
        }
    }

    /// The underlying store (for the daemon's run-start + recovery paths).
    #[must_use]
    pub fn store(&self) -> &SqliteStore {
        &self.store
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
    }

    /// Start a run: resolve its graph and execute from the start node to the
    /// first park (or completion), emitting the resulting state-change event.
    /// The daemon's run-start path (POST /runs) and the contract tests call this.
    ///
    /// # Errors
    /// [`CoreError`] on a store/handler/engine failure or an unresolved graph.
    pub async fn start_run(&self, run_id: &RunId) -> Result<RunProgress, CoreError> {
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

    async fn start_workflow(
        &self,
        flow: &str,
        run_id: &RunId,
        _context: Value,
    ) -> Result<RunProgress, CoreError> {
        let file = match flow {
            "draft" => "draft.dot",
            "execute" => "execute.dot",
            other => return Err(CoreError::Invariant(format!("unknown flow '{other}'"))),
        };
        // The initial `context` is accepted but not seeded in the MVP — the
        // shipped workflows use fixed paths, not context vars (real-env gap).
        let src = std::fs::read_to_string(self.workflows_dir.join(file))?;
        let sha = sha256_hex(src.as_bytes());
        run_repo::record_run(self.store.pool(), run_id, file, &sha).await?;
        self.start_run(run_id).await
    }

    async fn deliver(&self, event: EngineEvent) -> Result<RunProgress, CoreError> {
        // Resolve the run from the event; an unmatched event is replay-safe Ignored.
        let Some(run_id) = self.run_for_event(&event).await? else {
            return Ok(RunProgress::Ignored {
                reason: "no open park matches this event (unknown or already resolved)".to_string(),
            });
        };
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
        Ok(found.map(|(task_run_id, worktree)| TaskAssignment {
            task_run_id,
            // agent_id/spec_path/plan_path/context_pack have no columns (P0.9 D5
            // known gap); populated by the spawn context in the real-env path.
            agent_id: String::new(),
            worktree,
            spec_path: None,
            plan_path: None,
            context_pack: None,
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
