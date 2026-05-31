//! P0.7 7a Task 1: the `query_run` + `submit_outcome` MCP tools over the
//! `RunHost` seam (design §4.12.1). Test names match
//! `specs/surface/p70-mcp-tool-schemas.spec.md` and `p72-submit-outcome-idempotency.spec.md`.
//! Everything runs against a `FakeRunHost` — no real engine, MCP client, or socket.

use agentd_core::types::{NodeId, ReviewRunId, RunId, TaskRunId};
use agentd_core::{EngineEvent, ParkReason, RunProgress};

use agentd_surface::host::{RunSnapshot, TaskAssignment};
use agentd_surface::mcp_server::{dispatch, tool_descriptors};
use agentd_surface::test_support::FakeRunHost;
use agentd_surface::tools::assign_task::{AssignTaskInput, assign_task};
use agentd_surface::tools::check_inbox::{CheckInboxInput, check_inbox};
use agentd_surface::tools::query_run::{QueryRunInput, query_run};
use agentd_surface::tools::submit_outcome::{SubmitOutcomeInput, submit_outcome};
use agentd_surface::tools::submit_review::{SubmitReviewInput, submit_review};

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

// ---- submit_review + assign_task (p74) ------------------------------------

#[tokio::test]
async fn submit_review_records_and_reports_pending() {
    let host = FakeRunHost::new();
    host.push_progress(RunProgress::Parked {
        run_id: RunId::from_string("r1"),
        node_id: NodeId::parsed("review"),
        reason: ParkReason::ReviewVerdicts {
            review_run_id: ReviewRunId::from_string("rv1"),
            expected: 3,
        },
    });
    host.set_review_counts("rv1", (3, 1));

    let out = submit_review(
        &host,
        SubmitReviewInput {
            review_run_id: "rv1".to_string(),
            reviewer_id: "claude-sec".to_string(),
            verdict: "pass".to_string(),
            findings: vec![],
        },
    )
    .await
    .expect("submit_review ok");
    assert!(out.accepted);
    assert_eq!(out.fan_in_pending, 2, "expected 3 − got 1");

    match &host.delivered()[0] {
        EngineEvent::ReviewVerdictSubmitted { review_run_id, .. } => {
            assert_eq!(review_run_id.as_str(), "rv1");
        }
        other => panic!("expected ReviewVerdictSubmitted, got {other:?}"),
    }
}

#[tokio::test]
async fn submit_review_on_closed_review_is_already_submitted() {
    let host = FakeRunHost::new();
    host.push_progress(RunProgress::Ignored {
        reason: "review already aggregated".to_string(),
    });

    let err = submit_review(
        &host,
        SubmitReviewInput {
            review_run_id: "rv1".to_string(),
            reviewer_id: "claude-sec".to_string(),
            verdict: "pass".to_string(),
            findings: vec![],
        },
    )
    .await
    .expect_err("a closed review is already_submitted");
    assert_eq!(err.code(), "already_submitted");
}

#[tokio::test]
async fn assign_task_returns_open_task() {
    let host = FakeRunHost::new();
    host.set_task("r1", "implement", task("tr1"));

    let out = assign_task(
        &host,
        AssignTaskInput {
            run_id: "r1".to_string(),
            node_id: "implement".to_string(),
            agent_id: "impl-a".to_string(),
        },
    )
    .await
    .expect("assign ok");
    assert_eq!(out.task_run_id, "tr1");
    assert_eq!(out.worktree.as_deref(), Some("/wt"));
}

#[tokio::test]
async fn assign_task_other_agent_is_not_assigned() {
    let host = FakeRunHost::new();
    host.set_task("r1", "implement", task("tr1")); // assigned to impl-a

    let err = assign_task(
        &host,
        AssignTaskInput {
            run_id: "r1".to_string(),
            node_id: "implement".to_string(),
            agent_id: "impl-b".to_string(),
        },
    )
    .await
    .expect_err("another agent's task is not_assigned");
    assert_eq!(err.code(), "not_assigned");
}

#[tokio::test]
async fn assign_task_no_task_is_not_assigned() {
    let host = FakeRunHost::new();
    let err = assign_task(
        &host,
        AssignTaskInput {
            run_id: "r1".to_string(),
            node_id: "implement".to_string(),
            agent_id: "impl-a".to_string(),
        },
    )
    .await
    .expect_err("no task is not_assigned");
    assert_eq!(err.code(), "not_assigned");
}

// ---- check_inbox + dispatcher (p70) ---------------------------------------

#[tokio::test]
async fn check_inbox_returns_empty_v0() {
    let host = FakeRunHost::new();
    let out = check_inbox(
        &host,
        CheckInboxInput {
            agent_id: "impl-a".to_string(),
            drain: false,
        },
    )
    .await
    .expect("check_inbox ok");
    assert!(out.messages.is_empty(), "v0 inbox is empty");
}

#[tokio::test]
async fn dispatch_lists_five_tools() {
    let names: Vec<&str> = tool_descriptors().iter().map(|d| d.name).collect();
    assert_eq!(names.len(), 5);
    for expected in [
        "assign_task",
        "submit_outcome",
        "submit_review",
        "check_inbox",
        "query_run",
    ] {
        assert!(names.contains(&expected), "missing tool {expected}");
    }
}

#[tokio::test]
async fn dispatch_routes_to_handler() {
    let host = FakeRunHost::new();
    host.set_snapshot(
        "r1",
        RunSnapshot {
            status: "parked".to_string(),
            current_node: Some("review".to_string()),
            completed_nodes: vec![],
            context: json!({}),
        },
    );

    let out = dispatch(&host, "query_run", json!({"run_id": "r1"}))
        .await
        .expect("dispatch ok");
    assert_eq!(out["status"], "parked");
    assert_eq!(out["current_node"], "review");
}

#[tokio::test]
async fn dispatch_unknown_tool_is_error() {
    let host = FakeRunHost::new();
    let result = dispatch(&host, "no_such_tool", json!({})).await;
    assert!(result.is_err(), "an unregistered tool is an error");
}
