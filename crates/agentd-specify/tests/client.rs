use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use agentd_specify::test_support::{RecordingSpecifyClient, SpecifyCall};
use agentd_specify::{
    AcceptanceReport, DraftReceipt, DraftSpec, FrozenSpec, IssueContext, OfflineSpecify,
    SemanticEvent, SpecifyClient,
};
use serde_json::json;

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

fn issue_context() -> IssueContext {
    IssueContext {
        issue_id: "ACME-742".to_string(),
        title: "Add retry visibility".to_string(),
        body: "Need clearer retry events.".to_string(),
        labels: vec!["workflow".to_string()],
        github_number: Some(742),
    }
}

fn draft_spec() -> DraftSpec {
    DraftSpec {
        issue_id: "ACME-742".to_string(),
        spec_id: "spec-742".to_string(),
        content: "spec: task\nname: Retry visibility\n---\n".to_string(),
    }
}

fn draft_receipt() -> DraftReceipt {
    DraftReceipt {
        spec_id: "spec-742".to_string(),
        draft_id: "draft-1".to_string(),
    }
}

fn frozen_spec() -> FrozenSpec {
    FrozenSpec {
        spec_id: "spec-742".to_string(),
        version: "v1.0".to_string(),
        content: "frozen spec".to_string(),
    }
}

fn semantic_event() -> SemanticEvent {
    SemanticEvent {
        workflow_id: "wf-1".to_string(),
        kind: "workflow.started".to_string(),
        payload: json!({"run_id": "r1"}),
    }
}

fn acceptance_report() -> AcceptanceReport {
    AcceptanceReport {
        workflow_id: "wf-1".to_string(),
        accepted: true,
        pr_url: Some("https://github.com/acme/repo/pull/1".to_string()),
        summary: "accepted".to_string(),
    }
}

#[test]
fn specify_crate_is_private_workspace_member() {
    let workspace = read_repo_file("Cargo.toml");
    let manifest = read_repo_file("crates/agentd-specify/Cargo.toml");

    assert!(
        workspace.contains("\"crates/agentd-specify\""),
        "workspace should include agentd-specify: {workspace}"
    );
    assert!(
        manifest.contains("publish = false"),
        "agentd-specify should be private: {manifest}"
    );
}

#[tokio::test]
async fn offline_specify_preserves_standalone_mode() {
    let client = OfflineSpecify::new();

    for err in [
        client
            .pull_issue_context("ACME-742")
            .await
            .expect_err("offline issue context"),
        client
            .push_draft(draft_spec())
            .await
            .expect_err("offline draft push"),
        client
            .pull_frozen_spec("spec-742", "v1.0")
            .await
            .expect_err("offline frozen spec pull"),
    ] {
        assert_eq!(err.code(), "offline", "{err:?}");
    }

    client
        .report_event(semantic_event())
        .await
        .expect("offline reporting event is no-op");
    client
        .report_acceptance(acceptance_report())
        .await
        .expect("offline acceptance reporting is no-op");

    let manifest = read_repo_file("crates/agentd-specify/Cargo.toml");
    for forbidden in ["reqwest", "tokio-tungstenite", "url"] {
        assert!(
            !manifest.contains(forbidden),
            "offline seam must not depend on network transport {forbidden}: {manifest}"
        );
    }
}

#[tokio::test]
async fn specify_client_trait_is_object_safe() {
    let client: Arc<dyn SpecifyClient> = Arc::new(OfflineSpecify::new());

    let err = client
        .pull_issue_context("ACME-742")
        .await
        .expect_err("dyn OfflineSpecify returns offline");
    assert_eq!(err.code(), "offline");
}

#[tokio::test]
async fn recording_specify_client_captures_protocol_operations() {
    let issue = issue_context();
    let draft = draft_spec();
    let receipt = draft_receipt();
    let frozen = frozen_spec();
    let event = semantic_event();
    let report = acceptance_report();
    let client = RecordingSpecifyClient::new()
        .with_issue_context(issue.clone())
        .with_draft_receipt(receipt.clone())
        .with_frozen_spec(frozen.clone());

    assert_eq!(
        client
            .pull_issue_context("ACME-742")
            .await
            .expect("issue response"),
        issue
    );
    assert_eq!(
        client
            .push_draft(draft.clone())
            .await
            .expect("draft receipt"),
        receipt
    );
    assert_eq!(
        client
            .pull_frozen_spec("spec-742", "v1.0")
            .await
            .expect("frozen response"),
        frozen
    );
    client
        .report_event(event.clone())
        .await
        .expect("record event");
    client
        .report_acceptance(report.clone())
        .await
        .expect("record acceptance");

    assert_eq!(
        client.calls(),
        vec![
            SpecifyCall::PullIssueContext {
                issue_id: "ACME-742".to_string()
            },
            SpecifyCall::PushDraft { draft },
            SpecifyCall::PullFrozenSpec {
                spec_id: "spec-742".to_string(),
                version: "v1.0".to_string()
            },
            SpecifyCall::ReportEvent { event },
            SpecifyCall::ReportAcceptance { report },
        ]
    );
}

#[test]
fn readme_lists_agentd_specify_optional_adapter() {
    let readme = read_repo_file("README.md");

    assert!(readme.contains("agentd-specify"), "{readme}");
    assert!(
        readme.contains("optional Specify client") || readme.contains("optional Specify adapter"),
        "{readme}"
    );
}
