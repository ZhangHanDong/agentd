//! Tests for `agentd_core::ports` traits + `agentd_core::test_support` fakes.
//! Names match the spec `Test:` selectors. Requires the `test-support` feature,
//! enabled via agentd-core's self dev-dependency (see Cargo.toml).

use std::collections::HashMap;
use std::path::PathBuf;

use agentd_core::CoreError;
use agentd_core::ports::{
    AgentBackend, Clock, CommandOutput, CommandRunner, MempalClient, RunOpts, Store,
};
use agentd_core::test_support::{
    FakeBackend, FixedClock, InMemoryStore, MempalStub, RecordingCommandRunner,
};
use agentd_core::types::{
    AgentId, CliKind, LaunchStrategy, NodeId, Outcome, RunId, SpawnRequest, Status,
};

fn spawn_req() -> SpawnRequest {
    SpawnRequest {
        agent_id: AgentId::parsed("reviewer-1"),
        mxid: None,
        cli: CliKind::ClaudeCode,
        worktree: PathBuf::from("/tmp/wt"),
        initial_prompt: None,
        env_overrides: HashMap::new(),
        launch_strategy: LaunchStrategy::Direct,
    }
}

/// Exercises each port purely through a trait object, proving object safety.
async fn exercise_ports(
    backend: &dyn AgentBackend,
    runner: &dyn CommandRunner,
    store: &dyn Store,
    mempal: &dyn MempalClient,
    clock: &dyn Clock,
) {
    assert!(clock.now_unix() >= 0);
    runner
        .run("true", &[], RunOpts::default())
        .await
        .expect("run");
    mempal.search("q", "wing", "kind").await.expect("search");
    store
        .insert_run(&RunId::from_string("r_obj"), "sha")
        .await
        .expect("insert_run");
    backend.spawn(spawn_req()).await.expect("spawn");
}

#[tokio::test]
async fn ports_traits_are_object_safe() {
    let backend = FakeBackend::new();
    let runner = RecordingCommandRunner::new();
    let store = InMemoryStore::new();
    let mempal = MempalStub::new();
    let clock = FixedClock::new(0);
    exercise_ports(&backend, &runner, &store, &mempal, &clock).await;
    assert_eq!(backend.spawned().len(), 1, "spawn recorded via dyn ref");
}

#[tokio::test]
async fn in_memory_store_round_trips_run_and_outcome() {
    let store = InMemoryStore::new();
    let run = RunId::from_string("r1");
    let node = NodeId::parsed("impl");
    store.insert_run(&run, "sha1").await.expect("insert run");
    store
        .insert_node_outcome(&run, &node, &Outcome::success())
        .await
        .expect("insert outcome");
    let got = store.latest_outcome(&run, &node).await.expect("latest");
    assert_eq!(got.map(|o| o.status), Some(Status::Success));
    assert_eq!(store.count_attempts(&run, &node).await.expect("count"), 1);
}

#[tokio::test]
async fn in_memory_store_human_wait_answer_once_then_conflict() {
    let store = InMemoryStore::new();
    let run = RunId::from_string("r2");
    let node = NodeId::parsed("review");
    store.insert_run(&run, "s").await.expect("run");
    let wait_id = store
        .open_human_wait(&run, &node, "approve?")
        .await
        .expect("open");
    store
        .answer_human_wait(&wait_id, "approve", None)
        .await
        .expect("first answer ok");
    let err = store
        .answer_human_wait(&wait_id, "approve", None)
        .await
        .expect_err("second answer must conflict");
    assert!(matches!(err, CoreError::Store(_)), "got {err:?}");
}

#[tokio::test]
async fn in_memory_store_lookup_park_by_wait_id_returns_run_and_node() {
    let store = InMemoryStore::new();
    let run = RunId::from_string("r3");
    let node = NodeId::parsed("spec_review");
    store.insert_run(&run, "s").await.expect("run");
    let wait_id = store.open_human_wait(&run, &node, "?").await.expect("open");
    let parked = store
        .lookup_park_by_wait_id(&wait_id)
        .await
        .expect("lookup");
    assert_eq!(parked, Some((run, node)));
}

#[tokio::test]
async fn recording_command_runner_records_argv_and_returns_scripted_output() {
    let runner = RecordingCommandRunner::new();
    runner.push_output(Ok(CommandOutput {
        stdout: "hello\n".to_string(),
        stderr: String::new(),
        status: 0,
    }));
    let out = runner
        .run("echo", &["hello".to_string()], RunOpts::default())
        .await
        .expect("run");
    assert_eq!(out.stdout, "hello\n");
    assert_eq!(out.status, 0);
    let calls = runner.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].program, "echo");
    assert_eq!(calls[0].args, vec!["hello".to_string()]);
}

#[test]
fn fixed_clock_returns_set_time() {
    let clock = FixedClock::new(1000);
    assert_eq!(clock.now_unix(), 1000);
    clock.set(2000);
    assert_eq!(clock.now_unix(), 2000);
}
