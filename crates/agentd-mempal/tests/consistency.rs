//! P0.4 Task 4: the read-only consistency check (design §3.5). `check_against`
//! searches mempal for each expected drawer and reports the ones it does not
//! return. Test names match `specs/mempal/p31-consistency-check.spec.md`.

use std::sync::Arc;

use agentd_mempal::consistency::{ExpectedDrawer, check_against};
use agentd_mempal::test_support::RecordingToolCaller;
use agentd_mempal::{MempalConfig, MempalError, MempalMcpClient};

use serde_json::json;

fn drawer(query: &str) -> ExpectedDrawer {
    ExpectedDrawer {
        wing: "proj".to_string(),
        kind: "spec".to_string(),
        query: query.to_string(),
    }
}

#[tokio::test]
async fn test_consistency_check_reports_missing_drawers() {
    let rec = Arc::new(RecordingToolCaller::new());
    // first drawer present (a hit), second drawer missing (no hits).
    rec.push_result(Ok(
        json!({"hits": [{"drawer_id": "d1", "body": "present", "score": 0.9}]}),
    ));
    rec.push_result(Ok(json!({"hits": []})));
    let client = MempalMcpClient::new(rec, MempalConfig::default());

    let expected = vec![drawer("alpha"), drawer("beta")];
    let missing = check_against(&expected, &client).await;
    assert_eq!(
        missing,
        vec![expected[1].clone()],
        "only the empty-search drawer is reported missing"
    );
}

#[tokio::test]
async fn consistency_check_empty_when_all_present() {
    let rec = Arc::new(RecordingToolCaller::new());
    rec.push_result(Ok(
        json!({"hits": [{"drawer_id": "d1", "body": "x", "score": 0.5}]}),
    ));
    let client = MempalMcpClient::new(rec, MempalConfig::default());

    let missing = check_against(&[drawer("q")], &client).await;
    assert!(missing.is_empty());
}

#[tokio::test]
async fn consistency_search_failure_reports_missing() {
    let rec = Arc::new(RecordingToolCaller::new());
    rec.push_result(Err(MempalError::Transport("down".to_string())));
    let client = MempalMcpClient::new(rec, MempalConfig::default());

    let missing = check_against(&[drawer("q")], &client).await;
    assert_eq!(
        missing.len(),
        1,
        "a search failure is conservatively reported missing, without erroring"
    );
}
