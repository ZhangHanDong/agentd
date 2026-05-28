//! Tests for `agentd_core::engine::checkpoint`. Names match the spec `Test:` selectors.

use std::collections::BTreeMap;

use agentd_core::CoreError;
use agentd_core::engine::checkpoint::Checkpoint;
use agentd_core::types::{NodeId, RunContext, RunId};

fn sample() -> Checkpoint {
    let mut retry = BTreeMap::new();
    retry.insert(NodeId::parsed("impl"), 2_u32);
    let mut ctx = RunContext::new();
    ctx.set("answer", serde_json::Value::String("approve".to_string()));
    Checkpoint {
        run_id: RunId::from_string("r_test"),
        current_node: NodeId::parsed("review"),
        completed_nodes: vec![NodeId::parsed("start"), NodeId::parsed("impl")],
        retry_counts: retry,
        context_snapshot: ctx,
        workflow_sha: "abc".to_string(),
    }
}

#[test]
fn checkpoint_serialize_round_trips_through_json() {
    let cp = sample();
    let json = serde_json::to_string(&cp).expect("serialize");
    let back: Checkpoint = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, cp);
}

#[test]
fn checkpoint_write_is_atomic_via_temp_rename() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("checkpoint.json");
    let cp = sample();
    cp.write_atomic(&path).expect("write_atomic");
    assert!(path.exists(), "target file should exist");
    let tmp = dir.path().join("checkpoint.json.tmp");
    assert!(!tmp.exists(), "no leftover .tmp file");
    let back = Checkpoint::load(&path).expect("load");
    assert_eq!(back, cp);
}

#[test]
fn checkpoint_snapshot_includes_context_staged_before_park() {
    // Simulate a handler staging an answer into the context before parking.
    let mut ctx = RunContext::new();
    ctx.set("answer", serde_json::Value::String("changes".to_string()));
    let cp = Checkpoint {
        run_id: RunId::from_string("r"),
        current_node: NodeId::parsed("spec_review"),
        completed_nodes: vec![],
        retry_counts: BTreeMap::new(),
        context_snapshot: ctx,
        workflow_sha: "s".to_string(),
    };
    let json = serde_json::to_string(&cp).expect("serialize");
    let back: Checkpoint = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(
        back.context_snapshot.get("answer").and_then(|v| v.as_str()),
        Some("changes"),
        "staged context must survive the checkpoint"
    );
}

#[test]
fn checkpoint_resume_succeeds_when_sha_matches() {
    let cp = sample();
    cp.resume_guard("abc", false).expect("matching sha resumes");
}

#[test]
fn checkpoint_resume_errors_when_sha_changed_without_accept_flag() {
    let cp = sample();
    let err = cp
        .resume_guard("xyz", false)
        .expect_err("changed sha must error");
    assert!(matches!(err, CoreError::WorkflowShaChanged), "got {err:?}");
}

#[test]
fn checkpoint_resume_proceeds_with_accept_flag_and_logs_warning() {
    let cp = sample();
    cp.resume_guard("xyz", true)
        .expect("changed sha + accept flag resumes");
}
