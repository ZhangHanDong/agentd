//! P1: the live SSE tail (`live_event_stream`) — replay + live tail + seq-dedup +
//! Lagged→snapshot resync + terminal-close. Driven deterministically over a
//! PRE-LOADED broadcast receiver (no handler/socket timing). Names match
//! `specs/surface/p75-sse-live-tail.spec.md`.

use std::sync::Arc;

use agentd_core::types::RunId;
use agentd_surface::host::{EventRecord, LiveEvent, RunHost, RunSnapshot};
use agentd_surface::http::live_event_stream;
use agentd_surface::test_support::FakeRunHost;
use axum::response::{IntoResponse, sse::Sse};
use http_body_util::BodyExt;
use tokio::sync::broadcast;

fn live(seq: i64, kind: &str, payload: &str) -> LiveEvent {
    LiveEvent {
        run_id: "r1".to_string(),
        event: EventRecord {
            seq,
            kind: kind.to_string(),
            payload: payload.to_string(),
        },
    }
}

fn rec(seq: i64, kind: &str, payload: &str) -> EventRecord {
    EventRecord {
        seq,
        kind: kind.to_string(),
        payload: payload.to_string(),
    }
}

/// Serialize the live stream's SSE wire bytes (the stream must end — a terminal
/// event closes it — or this hangs).
async fn body_of(
    replay: Vec<EventRecord>,
    rx: broadcast::Receiver<LiveEvent>,
    host: Arc<dyn RunHost>,
) -> String {
    let stream = live_event_stream(replay, rx, host, RunId::from_string("r1"));
    let resp = Sse::new(stream).into_response();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect")
        .to_bytes();
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

#[tokio::test]
async fn live_stream_replays_then_tails_without_dup() {
    let (tx, rx) = broadcast::channel(16);
    // live: seq 2 overlaps the replay (must be deduped), seq 3 is new, seq 4 closes.
    tx.send(live(2, "node.parked", "LIVE2_SHOULD_BE_DEDUPED"))
        .unwrap();
    tx.send(live(3, "node.parked", "live3")).unwrap();
    tx.send(live(4, "run_finished", "{}")).unwrap();

    let replay = vec![
        rec(1, "run_parked", "replay1"),
        rec(2, "node.parked", "replay2"),
    ];
    let body = body_of(replay, rx, Arc::new(FakeRunHost::new())).await;

    assert!(body.contains("replay2"), "replayed seq 2: {body}");
    assert!(body.contains("live3"), "tailed the new seq 3: {body}");
    assert!(
        !body.contains("LIVE2_SHOULD_BE_DEDUPED"),
        "the seq-2 overlap is deduped: {body}"
    );
    assert!(
        body.contains("run_finished"),
        "closed on the terminal: {body}"
    );
}

#[tokio::test]
async fn live_stream_lag_sends_snapshot_resync() {
    // Capacity 2 + 4 sends => the receiver lags past its buffer on first recv.
    let (tx, rx) = broadcast::channel(2);
    tx.send(live(1, "node.parked", "a")).unwrap();
    tx.send(live(2, "node.parked", "b")).unwrap();
    tx.send(live(3, "node.parked", "c")).unwrap();
    tx.send(live(4, "run_finished", "{}")).unwrap();

    // The host serves the resync snapshot (non-terminal so the stream continues).
    let host = FakeRunHost::new();
    host.set_snapshot(
        "r1",
        RunSnapshot {
            status: "running".into(),
            current_node: Some("review".into()),
            completed_nodes: vec![],
            context: serde_json::json!({}),
        },
    );

    let body = body_of(Vec::new(), rx, Arc::new(host)).await;
    assert!(
        body.contains("state_resync"),
        "a lagging subscriber gets a snapshot resync, not an error: {body}"
    );
    assert!(
        body.contains("run_finished"),
        "still closes on the terminal: {body}"
    );
}

#[tokio::test]
async fn live_stream_terminal_closes() {
    let (tx, rx) = broadcast::channel(16);
    tx.send(live(1, "run_finished", "{}")).unwrap();
    // Reaching the assert at all proves the terminal closed the stream (else hang).
    let body = body_of(Vec::new(), rx, Arc::new(FakeRunHost::new())).await;
    assert!(
        body.contains("run_finished"),
        "yields the terminal then ends: {body}"
    );
}
