//! Worktree flow regression tests. The old P2 C1a Engine-level worktree
//! threading has been superseded; codergen now stages an allocated per-task_run
//! worktree into run context, and downstream handlers read that context.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agentd_core::CoreError;
use agentd_core::dot::parser;
use agentd_core::engine::{Engine, EngineEvent, ParkReason, RunProgress};
use agentd_core::graph::NodeGraph;
use agentd_core::handler::{HandlerRegistry, Ports};
use agentd_core::ports::WorktreeAllocator;
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
            agent_allocator: &agentd_core::ports::DirectAgentAllocator,
        }
    }

    fn engine<'a>(&'a self, graph: &'a NodeGraph) -> Engine<'a> {
        Engine::new(graph, &self.registry, self.ports(), "sha-test")
    }

    fn engine_with_allocator<'a>(
        &'a self,
        graph: &'a NodeGraph,
        allocator: Option<&'a dyn WorktreeAllocator>,
    ) -> Engine<'a> {
        Engine::new(graph, &self.registry, self.ports(), "sha-test")
            .with_worktree_allocator(allocator)
    }
}

#[derive(Debug)]
struct StaticAllocator {
    path: PathBuf,
}

#[async_trait::async_trait]
impl WorktreeAllocator for StaticAllocator {
    async fn allocate(&self, key: &str) -> Result<PathBuf, CoreError> {
        assert!(
            key.starts_with("tr_") || key.starts_with("tr"),
            "allocator key should be the task_run_id, got {key}"
        );
        Ok(self.path.clone())
    }

    async fn release(&self, _key: &str, _path: &std::path::Path) -> Result<(), CoreError> {
        Ok(())
    }
}

#[derive(Debug)]
struct FailingAllocator;

#[async_trait::async_trait]
impl WorktreeAllocator for FailingAllocator {
    async fn allocate(&self, _key: &str) -> Result<PathBuf, CoreError> {
        Err(CoreError::Backend("allocator boom".to_string()))
    }

    async fn release(&self, _key: &str, _path: &std::path::Path) -> Result<(), CoreError> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct RecordingAllocator {
    path: PathBuf,
    releases: Arc<Mutex<Vec<(String, PathBuf)>>>,
}

impl RecordingAllocator {
    fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            releases: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn releases(&self) -> Vec<(String, PathBuf)> {
        self.releases.lock().expect("release lock").clone()
    }
}

#[async_trait::async_trait]
impl WorktreeAllocator for RecordingAllocator {
    async fn allocate(&self, _key: &str) -> Result<PathBuf, CoreError> {
        Ok(self.path.clone())
    }

    async fn release(&self, key: &str, path: &std::path::Path) -> Result<(), CoreError> {
        self.releases
            .lock()
            .expect("release lock")
            .push((key.to_string(), path.to_path_buf()));
        Ok(())
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

fn repo_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn read_repo(path: &str) -> String {
    std::fs::read_to_string(repo_root().join(path))
        .unwrap_or_else(|e| panic!("read repo file {path}: {e}"))
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

#[test]
fn core_has_no_engine_or_handlerctx_worktree_threading() {
    let engine = read_repo("crates/agentd-core/src/engine/execute.rs");
    assert!(
        !engine.contains("with_worktree("),
        "Engine must not expose per-run with_worktree plumbing"
    );
    assert!(
        !engine.contains("worktree: Option<PathBuf>"),
        "Engine must not store a per-run worktree"
    );
    assert!(
        !engine.contains(".with_worktree("),
        "Engine must not thread worktree into HandlerCtx"
    );

    let handler = read_repo("crates/agentd-core/src/handler/mod.rs");
    assert!(
        !handler.contains("with_worktree("),
        "HandlerCtx must not expose with_worktree"
    );
    assert!(
        !handler.contains("pub fn worktree("),
        "HandlerCtx must not expose a worktree accessor"
    );
    assert!(
        !handler.contains("worktree: Option<&"),
        "HandlerCtx must not store an Engine-level worktree"
    );

    for path in [
        "crates/agentd-core/src/handler/tool.rs",
        "crates/agentd-core/src/handler/fan_out.rs",
        "crates/agentd-core/src/handler/codergen.rs",
    ] {
        let body = read_repo(path);
        assert!(
            !body.contains("ctx.worktree()"),
            "{path} must not read an Engine-level ctx.worktree fallback"
        );
    }
}

#[test]
fn worktree_threading_specs_mark_c1a_superseded() {
    for path in [
        "specs/core/p9-worktree-ctx-threading.spec.md",
        "specs/core/p10-tool-cmd-substitution.spec.md",
    ] {
        let body = read_repo(path);
        assert!(
            body.contains("superseded by the task-run worktree path"),
            "{path} must point readers to the task-run worktree replacement"
        );
    }
}

#[tokio::test]
async fn codergen_spawns_in_allocated_worktree() {
    let h = Harness::new();
    let g = graph();
    let wt = PathBuf::from("/tmp/wt-task-run");
    let allocator = StaticAllocator { path: wt.clone() };

    let parked = h
        .engine_with_allocator(&g, Some(&allocator))
        .execute(&h.run_id)
        .await
        .expect("execute");
    let tr = task_run_id(&parked);

    let spawned = h.backend.spawned();
    assert_eq!(spawned.len(), 1, "one agent spawned at impl");
    assert_eq!(
        spawned[0].worktree, wt,
        "allocated worktree reaches the SpawnRequest"
    );
    assert_eq!(
        h.store.task_worktree(&tr),
        Some(wt),
        "task_runs.worktree_path records the allocated worktree"
    );
}

// NOTE (design-faithful C1 redirect): the worktree threads to the AGENT spawn
// (codergen/fan_out `SpawnRequest.worktree`), NOT to tool-node cwd — tool nodes
// run in the daemon cwd and receive the worktree as a `--code <worktree>` arg
// via variable substitution (restored in R2). The former
// `engine_threads_worktree_to_tool_cwd` test was removed with that wiring.

#[tokio::test]
async fn engine_without_worktree_spawns_in_dot() {
    let h = Harness::new();
    let g = graph();
    let parked = h.engine(&g).execute(&h.run_id).await.expect("execute");
    let _ = task_run_id(&parked);
    let spawned = h.backend.spawned();
    assert_eq!(
        spawned[0].worktree,
        PathBuf::from("."),
        "no worktree preserves the current '.' default"
    );
}

#[tokio::test]
async fn codergen_without_allocator_spawns_in_dot() {
    let h = Harness::new();
    let g = graph();

    let parked = h
        .engine_with_allocator(&g, None)
        .execute(&h.run_id)
        .await
        .expect("execute");
    let tr = task_run_id(&parked);

    let spawned = h.backend.spawned();
    assert_eq!(spawned[0].worktree, PathBuf::from("."));
    assert_eq!(
        h.store.task_worktree(&tr),
        None,
        "without allocator, R3a remains inert and preserves no worktree path"
    );
}

#[tokio::test]
async fn tool_cmd_substitutes_worktree_and_context_var() {
    // R2/P103: a tool node's `${worktree}` and `${task_run_id}` resolve from
    // the context the codergen node staged after per-task_run allocation.
    let src = r#"digraph wt {
        "start" [shape=Mdiamond];
        "impl"  [handler=codergen, role="implementer"];
        "check" [handler=tool, cmd="verify --code ${worktree} --run ${task_run_id}"];
        "done"  [shape=Msquare];
        "start" -> "impl";
        "impl"  -> "check";
        "check" -> "done";
    }"#;
    let ast = parser::parse(src).expect("parse");
    let g = NodeGraph::from_ast(&ast).expect("validate");
    let h = Harness::new();
    let wt = PathBuf::from("/tmp/wt-X");
    let allocator = StaticAllocator { path: wt.clone() };

    let parked = h
        .engine_with_allocator(&g, Some(&allocator))
        .execute(&h.run_id)
        .await
        .expect("execute");
    let tr = task_run_id(&parked); // codergen staged this into the context
    let progress = h
        .engine_with_allocator(&g, Some(&allocator))
        .deliver_event(EngineEvent::AgentOutcomeSubmitted {
            task_run_id: tr.clone(),
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

    let call = h
        .runner
        .calls()
        .into_iter()
        .find(|c| c.program == "verify")
        .expect("the tool node `check` ran `verify`");
    assert!(
        call.args.contains(&wt.to_string_lossy().into_owned()),
        "${{worktree}} substituted to W: {:?}",
        call.args
    );
    assert!(
        call.args.contains(&tr.as_str().to_string()),
        "${{task_run_id}} substituted from context: {:?}",
        call.args
    );
    assert!(
        !call.args.iter().any(|a| a.contains("${")),
        "no literal '${{' remains: {:?}",
        call.args
    );
}

#[tokio::test]
async fn tool_resolves_allocated_worktree_via_context() {
    let src = r#"digraph wt {
        "start" [shape=Mdiamond];
        "impl"  [handler=codergen, role="implementer"];
        "check" [handler=tool, cmd="verify --code ${worktree} --run ${task_run_id}"];
        "done"  [shape=Msquare];
        "start" -> "impl";
        "impl"  -> "check";
        "check" -> "done";
    }"#;
    let ast = parser::parse(src).expect("parse");
    let g = NodeGraph::from_ast(&ast).expect("validate");
    let h = Harness::new();
    let wt = PathBuf::from("/tmp/wt-allocated");
    let allocator = StaticAllocator { path: wt.clone() };

    let parked = h
        .engine_with_allocator(&g, Some(&allocator))
        .execute(&h.run_id)
        .await
        .expect("execute");
    let tr = task_run_id(&parked);
    let progress = h
        .engine_with_allocator(&g, Some(&allocator))
        .deliver_event(EngineEvent::AgentOutcomeSubmitted {
            task_run_id: tr.clone(),
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

    let call = h
        .runner
        .calls()
        .into_iter()
        .find(|c| c.program == "verify")
        .expect("the tool node `check` ran `verify`");
    assert!(
        call.args.contains(&wt.to_string_lossy().into_owned()),
        "${{worktree}} substituted from the codergen-staged allocated path: {:?}",
        call.args
    );
    assert!(
        call.args.contains(&tr.as_str().to_string()),
        "${{task_run_id}} substituted from context: {:?}",
        call.args
    );
}

#[tokio::test]
async fn engine_releases_allocated_worktree_after_successful_terminal() {
    let h = Harness::new();
    let g = graph();
    let allocator = RecordingAllocator::new("/tmp/wt-release");

    let parked = h
        .engine_with_allocator(&g, Some(&allocator))
        .execute(&h.run_id)
        .await
        .expect("execute");
    let tr = task_run_id(&parked);
    let progress = h
        .engine_with_allocator(&g, Some(&allocator))
        .deliver_event(EngineEvent::AgentOutcomeSubmitted {
            task_run_id: tr.clone(),
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
    assert_eq!(
        allocator.releases(),
        vec![(tr.as_str().to_string(), PathBuf::from("/tmp/wt-release"))],
        "the allocated worktree is released after the downstream tool succeeds and the run finishes"
    );
}

#[tokio::test]
async fn engine_does_not_release_allocated_worktree_on_failure() {
    let src = r#"digraph wt {
        "start" [shape=Mdiamond];
        "impl"  [handler=codergen, role="implementer"];
        "check" [handler=tool, cmd="verify --code ${worktree}"];
        "done"  [shape=Msquare];
        "start" -> "impl";
        "impl"  -> "check";
        "check" -> "done" [condition="outcome=success"];
    }"#;
    let ast = parser::parse(src).expect("parse");
    let g = NodeGraph::from_ast(&ast).expect("validate");
    let h = Harness::new();
    let allocator = RecordingAllocator::new("/tmp/wt-failed");
    h.runner.push_output(Ok(agentd_core::ports::CommandOutput {
        stdout: String::new(),
        stderr: "verification failed".to_string(),
        status: 1,
    }));

    let parked = h
        .engine_with_allocator(&g, Some(&allocator))
        .execute(&h.run_id)
        .await
        .expect("execute");
    let tr = task_run_id(&parked);
    let progress = h
        .engine_with_allocator(&g, Some(&allocator))
        .deliver_event(EngineEvent::AgentOutcomeSubmitted {
            task_run_id: tr,
            outcome: Outcome::success(),
        })
        .await
        .expect("deliver");

    assert!(
        matches!(progress, RunProgress::Failed { .. }),
        "the run should fail when the downstream tool fails, got {progress:?}"
    );
    assert!(
        allocator.releases().is_empty(),
        "failed runs keep the worktree for debugging"
    );
}

#[tokio::test]
async fn codergen_allocator_failure_does_not_spawn() {
    let h = Harness::new();
    let g = graph();
    let err = h
        .engine_with_allocator(&g, Some(&FailingAllocator))
        .execute(&h.run_id)
        .await
        .expect_err("allocator failure should abort codergen");

    assert!(
        err.to_string().contains("allocator boom"),
        "error should surface allocator failure, got {err}"
    );
    assert!(
        h.backend.spawned().is_empty(),
        "codergen must not spawn when allocation fails"
    );
}
