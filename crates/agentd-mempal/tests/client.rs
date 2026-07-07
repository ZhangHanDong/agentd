//! Task 1: `MempalMcpClient` mapping the `MempalClient` port methods to mempal
//! MCP tool calls over the injected `McpToolCaller` seam (§4.12.2), and the
//! best-effort read timeout (§3.4). Test names match
//! `specs/mempal/p32-client-mcp-tools.spec.md` and `p33-pre-tools-best-effort.spec.md`.
//! Everything runs against a `RecordingToolCaller` — no real rmcp/mempal server.

use std::sync::Arc;
use std::time::Duration;

use agentd_core::CoreError;
use agentd_core::ports::MempalClient;

use agentd_mempal::test_support::RecordingToolCaller;
use agentd_mempal::{MempalConfig, MempalError, MempalMcpClient};

use serde_json::json;

fn client(rec: &Arc<RecordingToolCaller>, cfg: MempalConfig) -> MempalMcpClient {
    MempalMcpClient::new(rec.clone(), cfg)
}

// ---- p32: tool mapping + hit parsing --------------------------------------

#[tokio::test]
async fn search_issues_mempal_search_and_parses_hits() {
    let rec = Arc::new(RecordingToolCaller::new());
    rec.push_result(Ok(
        json!({"hits": [{"drawer_id": "d1", "body": "hello", "score": 0.9}]}),
    ));

    let hits = client(&rec, MempalConfig::default())
        .search("q", "proj", "spec")
        .await
        .expect("search ok");

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].drawer_id, "d1");
    assert_eq!(hits[0].body, "hello");

    let calls = rec.calls();
    assert_eq!(calls[0].0, "mempal_search");
    assert_eq!(calls[0].1["query"], "q");
    assert_eq!(calls[0].1["wing"], "proj");
    assert_eq!(calls[0].1["kind"], "spec");
}

#[tokio::test]
async fn ingest_issues_mempal_ingest() {
    let rec = Arc::new(RecordingToolCaller::new());
    rec.push_result(Ok(json!({"drawer_id": "d1"})));

    client(&rec, MempalConfig::default())
        .ingest("proj", "spec", "hello")
        .await
        .expect("ingest ok");

    let calls = rec.calls();
    assert_eq!(calls[0].0, "mempal_ingest");
    assert_eq!(calls[0].1["wing"], "proj");
    assert_eq!(calls[0].1["kind"], "spec");
    assert_eq!(calls[0].1["body"], "hello");
}

#[tokio::test]
async fn kg_add_issues_mempal_kg_add() {
    let rec = Arc::new(RecordingToolCaller::new());
    rec.push_result(Ok(json!({"triple_id": "t1"})));

    client(&rec, MempalConfig::default())
        .kg_add("s", "p", "o")
        .await
        .expect("kg_add ok");

    let calls = rec.calls();
    assert_eq!(calls[0].0, "mempal_kg");
    assert_eq!(calls[0].1["op"], "add");
    assert_eq!(calls[0].1["subject"], "s");
    assert_eq!(calls[0].1["predicate"], "p");
    assert_eq!(calls[0].1["object"], "o");
}

#[tokio::test]
async fn fact_check_issues_mempal_fact_check() {
    let rec = Arc::new(RecordingToolCaller::new());
    rec.push_result(Ok(
        json!({"issues": [{"drawer_id": "i1", "body": "contradiction", "score": 0.5}]}),
    ));

    let hits = client(&rec, MempalConfig::default())
        .fact_check("the sky is green")
        .await
        .expect("fact_check ok");

    assert_eq!(hits[0].drawer_id, "i1");
    let calls = rec.calls();
    assert_eq!(calls[0].0, "mempal_fact_check");
    assert_eq!(calls[0].1["text"], "the sky is green");
}

#[tokio::test]
async fn ingest_transport_failure_maps_to_core_mempal() {
    let rec = Arc::new(RecordingToolCaller::new());
    rec.push_result(Err(MempalError::Transport("boom".to_string())));

    let result = client(&rec, MempalConfig::default())
        .ingest("p", "k", "b")
        .await;
    assert!(matches!(result, Err(CoreError::Mempal(_))));
}

#[tokio::test]
async fn search_undecodable_payload_is_error() {
    let rec = Arc::new(RecordingToolCaller::new());
    rec.push_result(Ok(json!({"hits": [{"body": "no id"}]}))); // missing drawer_id

    let result = client(&rec, MempalConfig::default())
        .search("q", "w", "k")
        .await;
    assert!(matches!(result, Err(CoreError::Mempal(_))));
}

// ---- p33: best-effort read timeout ----------------------------------------

#[tokio::test]
async fn search_returns_hits_within_timeout() {
    let rec = Arc::new(RecordingToolCaller::new());
    rec.push_result(Ok(
        json!({"hits": [{"drawer_id": "d1", "body": "b", "score": 0.1}]}),
    ));

    let hits = client(&rec, MempalConfig::default())
        .search("q", "w", "k")
        .await
        .expect("search ok");
    assert_eq!(hits.len(), 1);
}

#[tokio::test]
async fn test_pre_tools_search_falls_back_to_empty_on_timeout() {
    let rec = Arc::new(RecordingToolCaller::new());
    rec.set_hang(true); // the caller never resolves

    let cfg = MempalConfig {
        pre_tools_timeout: Duration::ZERO,
    };
    let result = client(&rec, cfg).search("q", "w", "k").await;

    match &result {
        Err(CoreError::Mempal(_)) => {}
        other => panic!("expected a Mempal error within the timeout, got {other:?}"),
    }
    // The caller (mirroring codergen.rs) owns the empty fallback.
    assert!(result.unwrap_or_default().is_empty());
}

#[tokio::test]
async fn fact_check_times_out_to_error() {
    let rec = Arc::new(RecordingToolCaller::new());
    rec.set_hang(true);

    let cfg = MempalConfig {
        pre_tools_timeout: Duration::ZERO,
    };
    let result = client(&rec, cfg).fact_check("claim").await;
    assert!(matches!(result, Err(CoreError::Mempal(_))));
}
