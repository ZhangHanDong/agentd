//! P0.7 7a Task 1: the `query_run` + `submit_outcome` MCP tools over the
//! `RunHost` seam (design §4.12.1). Test names match
//! `specs/surface/p70-mcp-tool-schemas.spec.md` and `p72-submit-outcome-idempotency.spec.md`.
//! Everything runs against a `FakeRunHost` — no real engine, MCP client, or socket.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use agentd_core::types::{NodeId, ReviewRunId, RunId, TaskRunId, VerdictValue};
use agentd_core::{EngineEvent, ParkReason, RunProgress};

use agentd_surface::host::{
    AgentRegistration, GroupCreateInput, InboxMessage, RunHost, RunSnapshot, TaskAssignment,
};
use agentd_surface::mcp_server::{dispatch, tool_descriptors};
use agentd_surface::test_support::FakeRunHost;
use agentd_surface::tools::assign_task::{AssignTaskInput, assign_task};
use agentd_surface::tools::check_group::{CheckGroupInput, check_group};
use agentd_surface::tools::check_inbox::{CheckInboxInput, check_inbox};
use agentd_surface::tools::post::{PostInput, post as post_group};
use agentd_surface::tools::query_run::{QueryRunInput, query_run};
use agentd_surface::tools::send_message::{SendMessageInput, send_message};
use agentd_surface::tools::submit_human_answer::{SubmitHumanAnswerInput, submit_human_answer};
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

fn human_answer_input(wait_id: &str) -> SubmitHumanAnswerInput {
    SubmitHumanAnswerInput {
        wait_id: wait_id.to_string(),
        answer: "approve".to_string(),
        feedback: Some("looks good".to_string()),
    }
}

fn inbox_message(id: &str) -> InboxMessage {
    InboxMessage {
        id: id.to_string(),
        ts: 1_780_049_205_450,
        at: "2026-05-29T08:06:45.450Z".to_string(),
        time: "0s ago".to_string(),
        from: "alex".to_string(),
        to: "codex-worker".to_string(),
        message_type: "human".to_string(),
        priority: "normal".to_string(),
        summary: "please inspect the failing smoke".to_string(),
        full: "Please inspect the failing smoke and report the root cause.".to_string(),
        mentions: Vec::new(),
        attachments: Vec::new(),
        reply_to: None,
        group: None,
        source: "api".to_string(),
        source_room: None,
        sender_mxid: None,
        trust_level: Some("operator".to_string()),
        from_id: Some("alex".to_string()),
        schema: None,
    }
}

fn send_input(from_agent: &str, priority: Option<&str>) -> SendMessageInput {
    SendMessageInput {
        from_agent: Some(from_agent.to_string()),
        to: "codex-reviewer".to_string(),
        summary: "please review the smoke fix".to_string(),
        full: "Please review the smoke fix after the direct inbox change.".to_string(),
        message_type: None,
        priority: priority.map(str::to_string),
        reply_to: None,
        attachments: Vec::new(),
    }
}

async fn register_agent(host: &FakeRunHost, name: &str) {
    host.register_agent(AgentRegistration {
        name: name.to_string(),
        role: Some("agent".to_string()),
        capability: None,
        runtime: Some("codex".to_string()),
        model: None,
        tmux_target: None,
        home_dir: None,
        workdir: Some("/tmp/agentd-test".to_string()),
        state_dir: None,
        server: None,
        runtime_profile: json!({}),
    })
    .await
    .expect("register agent");
}

async fn create_group(host: &FakeRunHost, name: &str, members: &[&str]) {
    host.create_group(GroupCreateInput {
        name: name.to_string(),
        members: members.iter().map(ToString::to_string).collect(),
    })
    .await
    .expect("create group");
}

fn post_input(from_agent: &str, mentions: &[&str]) -> PostInput {
    PostInput {
        from_agent: Some(from_agent.to_string()),
        group: "factory".to_string(),
        summary: "group summary".to_string(),
        full: "group full".to_string(),
        message_type: None,
        priority: None,
        mentions: mentions.iter().map(ToString::to_string).collect(),
        reply_to: None,
        schema: None,
        attachments: Vec::new(),
    }
}

fn write_attachment(name: &str, bytes: &[u8]) -> String {
    let path = temp_attachment_path(name);
    fs::write(&path, bytes).expect("write attachment");
    path.to_string_lossy().to_string()
}

fn temp_attachment_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("agentd-p221-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir).expect("create temp attachment dir");
    dir.join(name)
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
            round: 1,
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
async fn submit_review_forwards_findings_to_engine_event() {
    let host = FakeRunHost::new();
    host.push_progress(RunProgress::Parked {
        run_id: RunId::from_string("r1"),
        node_id: NodeId::parsed("review"),
        reason: ParkReason::ReviewVerdicts {
            review_run_id: ReviewRunId::from_string("rv1"),
            expected: 1,
            round: 1,
        },
    });
    host.set_review_counts("rv1", (1, 1));

    let out = submit_review(
        &host,
        SubmitReviewInput {
            review_run_id: "rv1".to_string(),
            reviewer_id: "claude-sec".to_string(),
            verdict: "concern".to_string(),
            findings: vec![serde_json::json!({
                "path": "src/lib.rs",
                "body": "tighten error handling"
            })],
        },
    )
    .await
    .expect("submit_review ok");
    assert!(out.accepted);

    match &host.delivered()[0] {
        EngineEvent::ReviewVerdictSubmitted {
            review_run_id,
            reviewer_id,
            verdict,
            findings,
        } => {
            assert_eq!(review_run_id.as_str(), "rv1");
            assert_eq!(reviewer_id.as_str(), "claude-sec");
            assert_eq!(*verdict, VerdictValue::Fail);
            assert_eq!(
                findings,
                r#"[{"body":"tighten error handling","path":"src/lib.rs"}]"#
            );
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

#[tokio::test]
async fn submit_human_answer_delivers_and_reports_next() {
    let host = FakeRunHost::new();
    host.push_progress(RunProgress::Parked {
        run_id: RunId::from_string("r1"),
        node_id: NodeId::parsed("implement"),
        reason: ParkReason::AgentOutcome {
            task_run_id: TaskRunId::from_string("tr1"),
        },
    });

    let out = submit_human_answer(&host, human_answer_input("hw1"))
        .await
        .expect("submit human answer ok");
    assert!(out.accepted);
    assert_eq!(out.next_node.as_deref(), Some("implement"));

    let delivered = host.delivered();
    assert_eq!(delivered.len(), 1);
    match &delivered[0] {
        EngineEvent::HumanAnswered {
            wait_id,
            answer,
            feedback,
        } => {
            assert_eq!(wait_id, "hw1");
            assert_eq!(answer, "approve");
            assert_eq!(feedback.as_deref(), Some("looks good"));
        }
        other => panic!("expected HumanAnswered, got {other:?}"),
    }
}

#[tokio::test]
async fn submit_human_answer_stale_wait_is_already_submitted() {
    let host = FakeRunHost::new();
    host.push_progress(RunProgress::Ignored {
        reason: "wait already answered".to_string(),
    });

    let err = submit_human_answer(&host, human_answer_input("hw1"))
        .await
        .expect_err("closed wait is already_submitted");
    assert_eq!(err.code(), "already_submitted");
}

// ---- check_inbox + dispatcher (p70) ---------------------------------------

#[tokio::test]
async fn check_inbox_returns_durable_direct_messages_and_drains() {
    let host = FakeRunHost::new();
    host.push_inbox_message(inbox_message("msg_direct_1"));

    let preview = check_inbox(
        &host,
        CheckInboxInput {
            agent_id: "codex-worker".to_string(),
            drain: false,
        },
    )
    .await
    .expect("check_inbox ok");
    assert_eq!(preview.messages.len(), 1);
    assert_eq!(preview.dm.len(), 1);
    assert!(preview.group.is_empty(), "p217 only covers direct messages");
    assert_eq!(preview.messages[0]["id"], "msg_direct_1");
    assert_eq!(
        preview.messages[0]["summary"],
        "please inspect the failing smoke"
    );
    assert_eq!(preview.messages[0]["type"], "human");
    assert_eq!(preview.messages[0]["trustLevel"], "operator");

    let drained = check_inbox(
        &host,
        CheckInboxInput {
            agent_id: "codex-worker".to_string(),
            drain: true,
        },
    )
    .await
    .expect("drain ok");
    assert_eq!(drained.dm.len(), 1);
    assert_eq!(drained.dm[0]["id"], "msg_direct_1");

    let after_drain = check_inbox(
        &host,
        CheckInboxInput {
            agent_id: "codex-worker".to_string(),
            drain: false,
        },
    )
    .await
    .expect("after drain ok");
    assert!(after_drain.messages.is_empty(), "drain marks read");
    assert!(after_drain.dm.is_empty(), "dm mirrors messages for p217");
}

#[tokio::test]
async fn send_message_writes_direct_message_visible_through_check_inbox() {
    let names: Vec<&str> = tool_descriptors().iter().map(|d| d.name).collect();
    assert!(
        names.contains(&"send_message"),
        "send_message is registered"
    );

    let host = FakeRunHost::new();
    let out = send_message(&host, send_input("codex-worker", None))
        .await
        .expect("send_message ok");
    assert!(out.ok);
    assert_eq!(out.message["from"], "codex-worker");
    assert_eq!(out.message["to"], "codex-reviewer");
    assert_eq!(out.message["type"], "inform");
    assert_eq!(out.message["priority"], "normal");

    let inbox = check_inbox(
        &host,
        CheckInboxInput {
            agent_id: "codex-reviewer".to_string(),
            drain: false,
        },
    )
    .await
    .expect("target inbox");
    assert_eq!(inbox.dm.len(), 1);
    assert_eq!(inbox.dm[0]["summary"], "please review the smoke fix");
}

#[tokio::test]
async fn send_message_rejects_invalid_input_before_writing() {
    let host = FakeRunHost::new();

    let blank_sender = send_message(&host, send_input("   ", None))
        .await
        .expect_err("blank sender is bad_request");
    assert_eq!(blank_sender.code(), "bad_request");
    let after_blank = check_inbox(
        &host,
        CheckInboxInput {
            agent_id: "codex-reviewer".to_string(),
            drain: false,
        },
    )
    .await
    .expect("inbox after blank sender");
    assert!(after_blank.dm.is_empty());

    let bad_priority = send_message(&host, send_input("codex-worker", Some("panic")))
        .await
        .expect_err("bad priority is bad_request");
    assert_eq!(bad_priority.code(), "bad_request");
    let after_bad_priority = check_inbox(
        &host,
        CheckInboxInput {
            agent_id: "codex-reviewer".to_string(),
            drain: false,
        },
    )
    .await
    .expect("inbox after bad priority");
    assert!(after_bad_priority.dm.is_empty());
}

#[tokio::test]
async fn send_and_post_accept_readable_local_attachment_metadata() {
    let attachment_path = write_attachment("note.txt", b"hello agentd");

    let host = FakeRunHost::new();
    let mut direct = send_input("codex-worker", None);
    direct.attachments = vec![json!({
        "path": attachment_path,
        "mime": "text/plain",
        "kind": "file"
    })];
    send_message(&host, direct).await.expect("send_message ok");

    let inbox = check_inbox(
        &host,
        CheckInboxInput {
            agent_id: "codex-reviewer".to_string(),
            drain: false,
        },
    )
    .await
    .expect("direct inbox");
    let attachment = &inbox.dm[0]["attachments"][0];
    assert_eq!(attachment["name"], "note.txt");
    assert_eq!(attachment["mime"], "text/plain");
    assert_eq!(attachment["kind"], "file");
    assert_eq!(attachment["size"], 12);
    assert_eq!(attachment["staged"], false);
    assert_eq!(attachment["source_path"], attachment["path"]);

    register_agent(&host, "codex-a").await;
    register_agent(&host, "codex-b").await;
    create_group(&host, "factory", &["codex-a", "codex-b"]).await;
    let mut group = post_input("codex-a", &["codex-b"]);
    group.attachments = vec![json!({ "path": attachment["path"] })];
    post_group(&host, group).await.expect("post ok");

    let history = check_group(
        &host,
        CheckGroupInput {
            group: "factory".to_string(),
            agent_id: Some("codex-b".to_string()),
            limit: Some(10),
            unread_limit: None,
            read_all: Some(false),
        },
    )
    .await
    .expect("group history");
    assert_eq!(history.unread[0].attachments[0]["name"], "note.txt");
    assert_eq!(history.unread[0].attachments[0]["staged"], false);
}

#[tokio::test]
async fn send_and_post_reject_invalid_attachment_before_writing() {
    let host = FakeRunHost::new();
    let mut direct = send_input("codex-worker", None);
    direct.attachments = vec![json!({ "path": "/tmp/agentd-missing-p221" })];

    let err = send_message(&host, direct)
        .await
        .expect_err("missing attachment is bad_request");
    assert_eq!(err.code(), "bad_request");
    let inbox = check_inbox(
        &host,
        CheckInboxInput {
            agent_id: "codex-reviewer".to_string(),
            drain: false,
        },
    )
    .await
    .expect("inbox after failed send");
    assert!(inbox.dm.is_empty());

    register_agent(&host, "codex-a").await;
    register_agent(&host, "codex-b").await;
    create_group(&host, "factory", &["codex-a", "codex-b"]).await;
    let attachment_path = write_attachment("many.txt", b"many");
    let mut group = post_input("codex-a", &[]);
    group.attachments = (0..9)
        .map(|_| json!({ "path": attachment_path }))
        .collect::<Vec<_>>();

    let err = post_group(&host, group)
        .await
        .expect_err("too many attachments is bad_request");
    assert_eq!(err.code(), "bad_request");
    let history = check_group(
        &host,
        CheckGroupInput {
            group: "factory".to_string(),
            agent_id: Some("codex-b".to_string()),
            limit: Some(10),
            unread_limit: None,
            read_all: Some(false),
        },
    )
    .await
    .expect("group history after failed post");
    assert!(history.unread.is_empty());
}

#[tokio::test]
async fn dispatch_lists_and_routes_send_message_tool() {
    let names: Vec<&str> = tool_descriptors().iter().map(|d| d.name).collect();
    assert_eq!(names.len(), 9);
    for expected in [
        "assign_task",
        "submit_outcome",
        "submit_review",
        "submit_human_answer",
        "send_message",
        "post",
        "check_inbox",
        "check_group",
        "query_run",
    ] {
        assert!(names.contains(&expected), "missing tool {expected}");
    }

    let host = FakeRunHost::new();
    let out = dispatch(
        &host,
        "send_message",
        json!({
            "from": "codex-worker",
            "to": "codex-reviewer",
            "summary": "dispatch summary",
            "full": "dispatch full"
        }),
    )
    .await
    .expect("dispatch send_message ok");
    assert_eq!(out["ok"], true);
    assert_eq!(out["message"]["from"], "codex-worker");

    let inbox = check_inbox(
        &host,
        CheckInboxInput {
            agent_id: "codex-reviewer".to_string(),
            drain: false,
        },
    )
    .await
    .expect("target inbox");
    assert_eq!(inbox.dm.len(), 1);
    assert_eq!(inbox.dm[0]["summary"], "dispatch summary");
}

#[test]
fn dispatch_lists_nine_tools_with_submit_human_answer() {
    let names: Vec<&str> = tool_descriptors().iter().map(|tool| tool.name).collect();
    assert_eq!(names.len(), 9);
    assert!(names.contains(&"submit_human_answer"));
}

#[tokio::test]
async fn dispatch_lists_group_tools_after_p220() {
    let names: Vec<&str> = tool_descriptors().iter().map(|d| d.name).collect();
    for expected in [
        "post",
        "check_group",
        "send_message",
        "check_inbox",
        "query_run",
    ] {
        assert!(names.contains(&expected), "missing tool {expected}");
    }
}

#[tokio::test]
async fn post_group_message_mentions_member_and_warns_non_member() {
    let host = FakeRunHost::new();
    for agent in ["codex-a", "codex-b", "codex-c"] {
        register_agent(&host, agent).await;
    }
    create_group(&host, "factory", &["codex-a", "codex-b"]).await;

    let out = post_group(&host, post_input("codex-a", &["codex-b", "codex-c"]))
        .await
        .expect("post group message");
    assert!(out.ok);
    assert_eq!(out.delivery.target_kind, None);
    assert_eq!(out.delivery.suppressed, vec!["codex-c".to_string()]);
    assert!(
        out.warnings
            .iter()
            .any(|warning| warning["code"] == "mentions_not_in_group"),
        "warnings: {:?}",
        out.warnings
    );

    let b_inbox = check_inbox(
        &host,
        CheckInboxInput {
            agent_id: "codex-b".to_string(),
            drain: false,
        },
    )
    .await
    .expect("b inbox");
    assert_eq!(b_inbox.group.len(), 1);
    assert_eq!(b_inbox.group[0]["summary"], "group summary");
    assert_eq!(b_inbox.group[0]["group"], "factory");

    let c_inbox = check_inbox(
        &host,
        CheckInboxInput {
            agent_id: "codex-c".to_string(),
            drain: false,
        },
    )
    .await
    .expect("c inbox");
    assert!(c_inbox.group.is_empty());
}

#[tokio::test]
async fn check_group_previews_and_read_all_consumes_cursor() {
    let host = FakeRunHost::new();
    for agent in ["codex-a", "codex-b"] {
        register_agent(&host, agent).await;
    }
    create_group(&host, "factory", &["codex-a", "codex-b"]).await;
    for summary in ["one", "two", "three"] {
        let mut input = post_input("codex-a", &["codex-b"]);
        input.summary = summary.to_string();
        input.full = format!("full {summary}");
        post_group(&host, input).await.expect("post group");
    }

    let preview = check_group(
        &host,
        CheckGroupInput {
            group: "factory".to_string(),
            agent_id: Some("codex-b".to_string()),
            limit: Some(1),
            unread_limit: Some(2),
            read_all: Some(false),
        },
    )
    .await
    .expect("preview");
    assert_eq!(preview.unread_total, 3);
    assert_eq!(preview.unread_returned, 2);
    assert_eq!(preview.unread_omitted, 1);
    assert_eq!(preview.advance, "none");

    let preview_again = check_group(
        &host,
        CheckGroupInput {
            group: "factory".to_string(),
            agent_id: Some("codex-b".to_string()),
            limit: Some(1),
            unread_limit: Some(2),
            read_all: Some(false),
        },
    )
    .await
    .expect("preview again");
    assert_eq!(preview_again.unread_total, 3);

    let consumed = check_group(
        &host,
        CheckGroupInput {
            group: "factory".to_string(),
            agent_id: Some("codex-b".to_string()),
            limit: None,
            unread_limit: None,
            read_all: Some(true),
        },
    )
    .await
    .expect("consume");
    assert_eq!(consumed.advance, "all");

    let after = check_group(
        &host,
        CheckGroupInput {
            group: "factory".to_string(),
            agent_id: Some("codex-b".to_string()),
            limit: None,
            unread_limit: None,
            read_all: Some(false),
        },
    )
    .await
    .expect("after consume");
    assert_eq!(after.unread_total, 0);
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
