//! P2 Foundation B: the migration back-compat harness — proof that a NEW
//! migration preserves a DEPLOYED database's existing rows (the net the
//! fresh-state tests miss). Applies the REAL migration `.sql` files from disk via
//! raw SQL, seeds rows, then applies the migration under test and asserts the
//! rows survive. Names match `specs/store/p7-migration-backcompat.spec.md`.

use std::path::PathBuf;

use agentd_core::types::RunId;
use agentd_store::{SqliteStore, run_repo};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Row, SqlitePool};

fn migrations_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations")
}

/// A raw single-connection in-memory pool — NO migrator, so the harness controls
/// exactly which migration files are applied and in what order.
async fn raw_pool() -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory pool")
}

/// Apply one real migration file (read from disk at test time — the same file the
/// `sqlx::migrate!` embeds, never a hand-copied schema) via multi-statement raw SQL.
async fn apply(pool: &SqlitePool, file: &str) {
    let sql = std::fs::read_to_string(migrations_dir().join(file))
        .unwrap_or_else(|e| panic!("read migration {file}: {e}"));
    sqlx::raw_sql(&sql)
        .execute(pool)
        .await
        .unwrap_or_else(|e| panic!("apply migration {file}: {e}"));
}

#[tokio::test]
async fn migration_0002_preserves_existing_runs() {
    let pool = raw_pool().await;
    apply(&pool, "0001_init.sql").await;
    // Seed a run under the OLD (0001) schema — no worktree_path column exists yet.
    sqlx::query(
        "INSERT INTO runs (id, workflow_sha, status, started_at, last_heartbeat) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind("r1")
    .bind("sha")
    .bind("running")
    .bind(1_i64)
    .bind(1_i64)
    .execute(&pool)
    .await
    .expect("seed a pre-migration run");

    // The migration under test.
    apply(&pool, "0002_runs_worktree_path.sql").await;

    // The deployed row survives, and the new column defaulted to NULL.
    let row = sqlx::query("SELECT id, worktree_path FROM runs WHERE id = 'r1'")
        .fetch_one(&pool)
        .await
        .expect("the pre-existing run survives 0002");
    assert_eq!(row.get::<String, _>("id"), "r1");
    assert_eq!(
        row.get::<Option<String>, _>("worktree_path"),
        None,
        "a deployed run gets worktree_path NULL under 0002"
    );
}

#[tokio::test]
async fn migration_0002_adds_worktree_path_column() {
    // Via the REAL migrator (SqliteStore::connect runs 0001 + 0002).
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    run_repo::insert_run(store.pool(), &RunId::from_string("r1"), "sha")
        .await
        .expect("insert run");
    let wt: Option<String> = sqlx::query_scalar("SELECT worktree_path FROM runs WHERE id = 'r1'")
        .fetch_one(store.pool())
        .await
        .expect("worktree_path column exists after migration");
    assert_eq!(
        wt, None,
        "the migrated schema has worktree_path, default NULL"
    );
}

#[tokio::test]
async fn backcompat_harness_detects_row_loss() {
    // Proves the preservation check is NOT vacuous: a destructive statement (a
    // stand-in for a bad migration) makes the seeded row absent.
    let pool = raw_pool().await;
    apply(&pool, "0001_init.sql").await;
    sqlx::query("INSERT INTO runs (id, workflow_sha, status, started_at, last_heartbeat) VALUES ('r1','sha','running',1,1)")
        .execute(&pool)
        .await
        .expect("seed");
    sqlx::raw_sql("DELETE FROM runs WHERE id = 'r1';")
        .execute(&pool)
        .await
        .expect("destructive statement");
    let found = sqlx::query("SELECT id FROM runs WHERE id = 'r1'")
        .fetch_optional(&pool)
        .await
        .expect("query");
    assert!(
        found.is_none(),
        "the harness observes row loss — its preservation check is real"
    );
}
