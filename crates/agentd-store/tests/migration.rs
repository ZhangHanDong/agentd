//! Task 1: the schema migrates cleanly, is idempotent on reopen, and enforces
//! foreign keys. Names match the spec `Test:` selectors.

use agentd_store::SqliteStore;
use sqlx::Row;

async fn open_temp() -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("agentd.db");
    let store = SqliteStore::connect(&db).await.expect("connect + migrate");
    (store, dir)
}

#[tokio::test]
async fn migration_creates_expected_tables() {
    let (store, _dir) = open_temp().await;
    let rows = sqlx::query("SELECT name FROM sqlite_master WHERE type='table'")
        .fetch_all(store.pool())
        .await
        .expect("query tables");
    let tables: Vec<String> = rows.iter().map(|r| r.get::<String, _>("name")).collect();
    for expected in [
        "projects",
        "agents",
        "issues",
        "runs",
        "node_outcomes",
        "checkpoints",
        "artifacts",
        "task_runs",
        "review_runs",
        "review_verdicts",
        "review_worktrees",
        "human_waits",
        "mempal_outbox",
        "matrix_events",
        "events",
        "schema_meta",
    ] {
        assert!(
            tables.contains(&expected.to_string()),
            "missing table '{expected}'; got {tables:?}"
        );
    }
}

#[tokio::test]
async fn migration_is_idempotent_on_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("agentd.db");
    {
        let _s1 = SqliteStore::connect(&db).await.expect("first open");
    }
    let s2 = SqliteStore::connect(&db)
        .await
        .expect("reopen applies no new migrations");
    let version: String = sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = 'version'")
        .fetch_one(s2.pool())
        .await
        .expect("schema version row");
    assert_eq!(version, "2");
}

#[tokio::test]
async fn foreign_keys_are_enforced() {
    let (store, _dir) = open_temp().await;
    // node_outcomes.run_id REFERENCES runs(id); an orphan insert must be rejected
    // (proves `.foreign_keys(true)` is active on the connection).
    let result = sqlx::query(
        "INSERT INTO node_outcomes \
         (run_id, node_id, attempt, status, context_delta, artifacts, started_at, finished_at) \
         VALUES ('ghost-run', 'n', 1, 'success', '{}', '[]', 0, 0)",
    )
    .execute(store.pool())
    .await;
    assert!(
        result.is_err(),
        "FK to runs(id) should reject an orphan node_outcome"
    );
}
