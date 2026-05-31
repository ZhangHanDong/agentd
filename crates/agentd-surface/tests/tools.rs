//! P0.7 7a Task 1: the `query_run` + `submit_outcome` MCP tools over the
//! `RunHost` seam (design §4.12.1). Test names match
//! `specs/surface/p70-mcp-tool-schemas.spec.md` and `p72-submit-outcome-idempotency.spec.md`.
//! Everything runs against a `FakeRunHost` — no real engine, MCP client, or socket.

use agentd_core::types::{NodeId, RunId, TaskRunId};
use agentd_core::{EngineEvent, ParkReason, RunProgress};

use agentd_surface::host::{RunSnapshot, TaskAssignment};
use agentd_surface::test_support::FakeRunHost;
use agentd_surface::tools::query_run::{QueryRunInput, query_run};
use agentd_surface::tools::submit_outcome::{SubmitOutcomeInput, submit_outcome};

use serde_json::json;

fn task(id: &str) -> TaskAssignment {
    TaskAssignment {
        task_run_id: TaskRunId::from_string(id),
        agent_id: "impl-a".to_string(),
        worktree: Some("/wt".to_string()),
        spec_path: None,
        plan_path: None,
        context_pack: None,
    }
}

fn submit_input(run: &str, node: &str) -> SubmitOutcomeInput {
    SubmitOutcomeInput {
        run_id: run.to_string(),
        node_id: node.to_string(),
        attempt: 1,
        status: "success".to_string(),
        context_updates: serde_json::Map::new(),
        preferred_label: None,
        suggested_next: vec![],
    }
}

// ---- query_run (p70) ------------------------------------------------------

#[tokio::test]
async fn query_run_returns_snapshot() {
    let host = FakeRunHost::new();
    host.set_snapshot(
        "r1",
        RunSnapshot {
            status: "parked".to_string(),
            current_node: Some("review".to_string()),
            completed_nodes: vec!["start".to_string(), "implement".to_string()],
            context: json!({"k": "v"}),
        },
    );

    let out = query_run(
        &host,
        QueryRunInput {
            run_id: "r1".to_string(),
        },
    )
    .await
    .expect("query ok");
    assert_eq!(out.status, "parked");
    assert_eq!(out.current_node.as_deref(), Some("review"));
    assert_eq!(
        out.completed_nodes,
        vec!["start".to_string(), "implement".to_string()]
    );
}

#[tokio::test]
async fn query_run_unknown_is_not_found() {
    let host = FakeRunHost::new();
    let err = query_run(
        &host,
        QueryRunInput {
            run_id: "ghost".to_string(),
        },
    )
    .await
    .expect_err("unknown run is not_found");
    assert_eq!(err.code(), "not_found");
}

// ---- submit_outcome (p70 + p72) -------------------------------------------

#[tokio::test]
async fn submit_outcome_delivers_and_reports_next() {
    let host = FakeRunHost::new();
    host.set_task("r1", "implement", task("tr1"));
    host.push_progress(RunProgress::Parked {
        run_id: RunId::from_string("r1"),
        node_id: NodeId::parsed("review"),
        reason: ParkReason::AgentOutcome {
            task_run_id: TaskRunId::from_string("tr2"),
        },
    });

    let out = submit_outcome(&host, submit_input("r1", "implement"))
        .await
        .expect("submit ok");
    assert!(out.recorded);
    assert_eq!(out.next_node.as_deref(), Some("review"));

    let delivered = host.delivered();
    assert_eq!(delivered.len(), 1);
    match &delivered[0] {
        EngineEvent::AgentOutcomeSubmitted { task_run_id, .. } => {
            assert_eq!(task_run_id.as_str(), "tr1");
        }
        other => panic!("expected AgentOutcomeSubmitted, got {other:?}"),
    }
}

#[tokio::test]
async fn submit_outcome_no_task_is_not_assigned() {
    let host = FakeRunHost::new();
    let err = submit_outcome(&host, submit_input("r1", "implement"))
        .await
        .expect_err("no task is not_assigned");
    assert_eq!(err.code(), "not_assigned");
    assert!(host.delivered().is_empty(), "no event delivered");
}

#[tokio::test]
async fn submit_outcome_stale_park_is_stale_attempt() {
    let host = FakeRunHost::new();
    host.set_task("r1", "implement", task("tr1"));
    host.push_progress(RunProgress::Ignored {
        reason: "park already moved".to_string(),
    });

    let err = submit_outcome(&host, submit_input("r1", "implement"))
        .await
        .expect_err("a moved park is stale_attempt");
    assert_eq!(err.code(), "stale_attempt");
}

#[tokio::test]
async fn submit_outcome_finished_has_no_next() {
    let host = FakeRunHost::new();
    host.set_task("r1", "implement", task("tr1"));
    host.push_progress(RunProgress::Finished {
        run_id: RunId::from_string("r1"),
    });

    let out = submit_outcome(&host, submit_input("r1", "implement"))
        .await
        .expect("submit ok");
    assert!(out.recorded);
    assert_eq!(out.next_node, None);
}
