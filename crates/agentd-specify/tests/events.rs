use std::fs;
use std::path::PathBuf;

use agentd_specify::test_support::{RecordingSpecifyClient, SpecifyCall};
use agentd_specify::{
    AcceptanceReport, AgentdEventRef, DraftReceipt, DraftSpec, FrozenSpec, IssueContext,
    OfflineSpecify, SemanticEvent, SpecifyClient, SpecifyError, map_agentd_event,
    report_agentd_event,
};
use serde_json::json;

#[derive(Debug)]
struct FailingReportClient;

#[async_trait::async_trait]
impl SpecifyClient for FailingReportClient {
    async fn pull_issue_context(&self, _issue_id: &str) -> Result<IssueContext, SpecifyError> {
        panic!("pull_issue_context is not used by event reporting")
    }

    async fn push_draft(&self, _draft: DraftSpec) -> Result<DraftReceipt, SpecifyError> {
        panic!("push_draft is not used by event reporting")
    }

    async fn pull_frozen_spec(
        &self,
        _spec_id: &str,
        _version: &str,
    ) -> Result<FrozenSpec, SpecifyError> {
        panic!("pull_frozen_spec is not used by event reporting")
    }

    async fn report_event(&self, _event: SemanticEvent) -> Result<(), SpecifyError> {
        Err(SpecifyError::Transport("report failed".to_string()))
    }

    async fn report_acceptance(&self, _report: AcceptanceReport) -> Result<(), SpecifyError> {
        panic!("report_acceptance is not used by event reporting")
    }
}

fn repo_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn read_repo_file(path: &str) -> String {
    fs::read_to_string(repo_root().join(path)).unwrap_or_else(|err| {
        panic!("read {path}: {err}");
    })
}

fn event<'a>(run_id: &'a str, seq: i64, kind: &'a str, payload: &'a str) -> AgentdEventRef<'a> {
    AgentdEventRef {
        run_id,
        seq,
        kind,
        payload,
    }
}

#[test]
fn run_parked_maps_to_agent_blocked_with_node_and_round() {
    let mapped = map_agentd_event(
        "wf1",
        event("r1", 7, "run_parked", r#"{"node":"review","round":1}"#),
    )
    .expect("valid event")
    .expect("mapped event");

    assert_eq!(mapped.workflow_id, "wf1");
    assert_eq!(mapped.kind, "agent.blocked");
    assert_eq!(
        mapped.payload,
        json!({
            "run_id": "r1",
            "seq": 7,
            "agentd_event_kind": "run_parked",
            "payload": {
                "node": "review",
                "round": 1
            }
        })
    );
}

#[test]
fn run_finished_maps_to_workflow_finished() {
    let mapped = map_agentd_event("wf1", event("r1", 8, "run_finished", "{}"))
        .expect("valid event")
        .expect("mapped event");

    assert_eq!(mapped.workflow_id, "wf1");
    assert_eq!(mapped.kind, "workflow.finished");
    assert_eq!(
        mapped.payload,
        json!({
            "run_id": "r1",
            "seq": 8,
            "agentd_event_kind": "run_finished",
            "payload": {}
        })
    );
}

#[test]
fn run_failed_maps_to_workflow_failed_with_reason() {
    let mapped = map_agentd_event("wf1", event("r1", 9, "run_failed", r#"{"reason":"boom"}"#))
        .expect("valid event")
        .expect("mapped event");

    assert_eq!(mapped.workflow_id, "wf1");
    assert_eq!(mapped.kind, "workflow.failed");
    assert_eq!(
        mapped.payload,
        json!({
            "run_id": "r1",
            "seq": 9,
            "agentd_event_kind": "run_failed",
            "payload": {
                "reason": "boom"
            }
        })
    );
}

#[test]
fn unknown_agentd_event_kind_is_ignored() {
    let mapped = map_agentd_event("wf1", event("r1", 10, "state_resync", "{not-json"))
        .expect("unknown event is ignored before payload decode");

    assert_eq!(mapped, None);
}

#[test]
fn invalid_event_payload_is_decode_error() {
    let err = map_agentd_event("wf1", event("r1", 11, "run_parked", "{not-json"))
        .expect_err("known event payload must decode");

    assert_eq!(err.code(), "decode");
    assert!(matches!(err, SpecifyError::Decode(_)), "{err:?}");
}

#[tokio::test]
async fn mapped_agentd_event_is_reported_through_specify_client() {
    let client = RecordingSpecifyClient::new();

    let reported = report_agentd_event(
        &client,
        "wf1",
        event("r1", 7, "run_parked", r#"{"node":"review","round":1}"#),
    )
    .await
    .expect("mapped event reports");

    assert!(reported);
    assert_eq!(
        client.calls(),
        vec![SpecifyCall::ReportEvent {
            event: SemanticEvent {
                workflow_id: "wf1".to_string(),
                kind: "agent.blocked".to_string(),
                payload: json!({
                    "run_id": "r1",
                    "seq": 7,
                    "agentd_event_kind": "run_parked",
                    "payload": {
                        "node": "review",
                        "round": 1
                    }
                }),
            },
        }]
    );
}

#[tokio::test]
async fn unknown_agentd_event_is_not_reported() {
    let client = RecordingSpecifyClient::new();

    let reported = report_agentd_event(&client, "wf1", event("r1", 10, "state_resync", "{bad"))
        .await
        .expect("unknown event is ignored");

    assert!(!reported);
    assert!(client.calls().is_empty(), "{:?}", client.calls());
}

#[tokio::test]
async fn invalid_event_payload_is_not_reported() {
    let client = RecordingSpecifyClient::new();

    let err = report_agentd_event(&client, "wf1", event("r1", 11, "run_parked", "{not-json"))
        .await
        .expect_err("known event payload must decode");

    assert_eq!(err.code(), "decode");
    assert!(client.calls().is_empty(), "{:?}", client.calls());
}

#[tokio::test]
async fn client_report_event_error_propagates_after_mapping() {
    let client = FailingReportClient;

    let err = report_agentd_event(&client, "wf1", event("r1", 12, "run_finished", "{}"))
        .await
        .expect_err("client report error propagates");

    assert_eq!(err.code(), "transport");
    assert!(matches!(err, SpecifyError::Transport(message) if message == "report failed"));
}

#[tokio::test]
async fn offline_event_reporting_preserves_standalone_noop() {
    let client = OfflineSpecify::new();

    let reported = report_agentd_event(
        &client,
        "wf1",
        event("r1", 13, "run_failed", r#"{"reason":"boom"}"#),
    )
    .await
    .expect("offline event reporting remains a no-op success");

    assert!(reported);
}

#[test]
fn semantic_mapping_keeps_runtime_wiring_out_of_specify_crate() {
    let workspace = read_repo_file("Cargo.toml");
    let manifest = read_repo_file("crates/agentd-specify/Cargo.toml");
    let source = read_repo_file("crates/agentd-specify/src/events.rs");

    assert!(
        workspace.contains("\"crates/agentd-specify\""),
        "workspace should include agentd-specify: {workspace}"
    );
    for forbidden in [
        "agentd-surface",
        "agentd-bin",
        "reqwest",
        "tokio-tungstenite",
    ] {
        assert!(
            !manifest.contains(forbidden),
            "agentd-specify must not depend on {forbidden}: {manifest}"
        );
    }
    assert!(
        !source.contains("EventRecord"),
        "mapper source must stay decoupled from agentd-surface EventRecord: {source}"
    );
}

#[test]
fn event_reporting_helper_keeps_runtime_wiring_out() {
    let manifest = read_repo_file("crates/agentd-specify/Cargo.toml");
    let source = read_repo_file("crates/agentd-specify/src/events.rs");

    for forbidden in [
        "agentd-surface",
        "agentd-bin",
        "reqwest",
        "tokio-tungstenite",
    ] {
        assert!(
            !manifest.contains(forbidden),
            "agentd-specify must not depend on {forbidden}: {manifest}"
        );
    }
    assert!(
        !source.contains("EventRecord"),
        "helper source must stay decoupled from agentd-surface EventRecord: {source}"
    );
}
