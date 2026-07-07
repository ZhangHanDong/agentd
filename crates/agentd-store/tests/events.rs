//! P0.7 7b Task 4: `event_repo` — the append-only event log + cursor read that
//! backs SSE replay (design §4 events). Test names match
//! `specs/surface/p71-sse-event-replay.spec.md`.

use agentd_core::types::RunId;
use agentd_store::{SqliteStore, event_repo, run_repo};

async fn store() -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let s = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    (s, dir)
}

#[tokio::test]
async fn events_append_and_read_from_zero() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r1");
    run_repo::insert_run(s.pool(), &run, "sha")
        .await
        .expect("run");

    let s1 = event_repo::append(s.pool(), &run, "run.started", "{}")
        .await
        .expect("append 1");
    let s2 = event_repo::append(s.pool(), &run, "node.parked", r#"{"node":"review"}"#)
        .await
        .expect("append 2");
    let s3 = event_repo::append(s.pool(), &run, "run.finished", "{}")
        .await
        .expect("append 3");
    assert!(s1 < s2 && s2 < s3, "seq strictly increasing");

    let rows = event_repo::read_from(s.pool(), &run, 0)
        .await
        .expect("read");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].kind, "run.started");
    assert_eq!(rows[2].kind, "run.finished");
    assert!(rows[0].seq < rows[1].seq && rows[1].seq < rows[2].seq);
}

#[tokio::test]
async fn events_read_from_cursor_skips_earlier() {
    let (s, _d) = store().await;
    let run = RunId::from_string("r1");
    run_repo::insert_run(s.pool(), &run, "sha")
        .await
        .expect("run");

    let first = event_repo::append(s.pool(), &run, "a", "{}")
        .await
        .expect("a");
    event_repo::append(s.pool(), &run, "b", "{}")
        .await
        .expect("b");
    event_repo::append(s.pool(), &run, "c", "{}")
        .await
        .expect("c");

    let rows = event_repo::read_from(s.pool(), &run, first)
        .await
        .expect("read");
    assert_eq!(rows.len(), 2, "the cursor skips the first event");
    assert_eq!(rows[0].kind, "b");
    assert_eq!(rows[1].kind, "c");
}

#[tokio::test]
async fn events_read_from_other_run_is_empty() {
    let (s, _d) = store().await;
    let r1 = RunId::from_string("r1");
    run_repo::insert_run(s.pool(), &r1, "sha")
        .await
        .expect("run");
    event_repo::append(s.pool(), &r1, "a", "{}")
        .await
        .expect("a");

    let rows = event_repo::read_from(s.pool(), &RunId::from_string("r2"), 0)
        .await
        .expect("read");
    assert!(rows.is_empty());
}

#[tokio::test]
async fn events_append_unknown_run_is_error() {
    let (s, _d) = store().await;
    let result = event_repo::append(s.pool(), &RunId::from_string("ghost"), "a", "{}").await;
    assert!(
        result.is_err(),
        "appending for a non-existent run violates the FK"
    );
}
