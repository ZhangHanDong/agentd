//! P2 C1a: the engine threads an optional per-run worktree down to the spawn +
//! tool handlers. Names match `specs/core/p9-worktree-ctx-threading.spec.md`.
//! `with_worktree(Some(W))` → the codergen `SpawnRequest.worktree` and the tool
//! `RunOpts.cwd`; the default `None` reproduces today's `"."`.

use std::path::PathBuf;

use agentd_core::dot::parser;
use agentd_core::engine::{Engine, EngineEvent, ParkReason, RunProgress};
use agentd_core::graph::NodeGraph;
use agentd_core::handler::{HandlerRegistry, Ports};
use agentd_core::test_support::{
    FakeBackend, FixedClock, InMemoryStore, MempalStub, RecordingCommandRunner,
};
use agentd_core::types::{Outcome, RunId, TaskRunId};

struct Harness {
    run_id: RunId,
    registry: HandlerRegistry,
    backend: FakeBackend,
    runner: RecordingCommandRunner,
    store: InMemoryStore,
    mempal: MempalStub,
    clock: FixedClock,
}

impl Harness {
    fn new() -> Self {
        Self {
            run_id: RunId::from_string("run-1"),
            registry: HandlerRegistry::with_builtins(),
            backend: FakeBackend::new(),
            runner: RecordingCommandRunner::new(),
            store: InMemoryStore::new(),
            mempal: MempalStub::new(),
            clock: FixedClock::new(0),
        }
    }

    fn ports(&self) -> Ports<'_> {
        Ports {
            backend: &self.backend,
            runner: &self.runner,
            store: &self.store,
            mempal: &self.mempal,
            clock: &self.clock,
        }
    }

    fn engine<'a>(&'a self, graph: &'a NodeGraph, worktree: Option<PathBuf>) -> Engine<'a> {
        Engine::new(graph, &self.registry, self.ports(), "sha-test").with_worktree(worktree)
    }
}

fn graph() -> NodeGraph {
    let src = r#"digraph wt {
        "start" [shape=Mdiamond];
        "impl"  [handler=codergen, role="implementer"];
        "check" [handler=tool, cmd="echo ok"];
        "done"  [shape=Msquare];
        "start" -> "impl";
        "impl"  -> "check";
        "check" -> "done";
    }"#;
    let ast = parser::parse(src).expect("dot parse");
    NodeGraph::from_ast(&ast).expect("validate")
}

fn task_run_id(progress: &RunProgress) -> TaskRunId {
    match progress {
        RunProgress::Parked {
            reason: ParkReason::AgentOutcome { task_run_id },
            ..
        } => task_run_id.clone(),
        other => panic!("expected AgentOutcome park at impl, got {other:?}"),
    }
}

#[tokio::test]
async fn engine_threads_worktree_to_spawn_request() {
    let h = Harness::new();
    let g = graph();
    let wt = PathBuf::from("/tmp/wt-run-1");
    let parked = h
        .engine(&g, Some(wt.clone()))
        .execute(&h.run_id)
        .await
        .expect("execute");
    let _ = task_run_id(&parked); // parked at codergen
    let spawned = h.backend.spawned();
    assert_eq!(spawned.len(), 1, "one agent spawned at impl");
    assert_eq!(
        spawned[0].worktree, wt,
        "the threaded worktree reaches the SpawnRequest"
    );
}

#[tokio::test]
async fn engine_threads_worktree_to_tool_cwd() {
    let h = Harness::new();
    let g = graph();
    let wt = PathBuf::from("/tmp/wt-run-1");
    // impl parks; deliver its success so the tool node `check` runs.
    let parked = h
        .engine(&g, Some(wt.clone()))
        .execute(&h.run_id)
        .await
        .expect("execute");
    let tr = task_run_id(&parked);
    let progress = h
        .engine(&g, Some(wt.clone()))
        .deliver_event(EngineEvent::AgentOutcomeSubmitted {
            task_run_id: tr,
            outcome: Outcome::success(),
        })
        .await
        .expect("deliver");
    assert_eq!(
        progress,
        RunProgress::Finished {
            run_id: h.run_id.clone()
        }
    );
    let echo = h
        .runner
        .calls()
        .into_iter()
        .find(|c| c.program == "echo")
        .expect("the tool node `check` ran `echo`");
    assert_eq!(
        echo.cwd,
        Some(wt),
        "the threaded worktree becomes the tool node's cwd"
    );
}

#[tokio::test]
async fn engine_without_worktree_spawns_in_dot() {
    let h = Harness::new();
    let g = graph();
    // No with_worktree(Some(..)) — the default None must reproduce today's ".".
    let parked = h
        .engine(&g, None)
        .execute(&h.run_id)
        .await
        .expect("execute");
    let _ = task_run_id(&parked);
    let spawned = h.backend.spawned();
    assert_eq!(
        spawned[0].worktree,
        PathBuf::from("."),
        "no worktree preserves the current '.' default"
    );
}
