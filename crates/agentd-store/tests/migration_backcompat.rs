//! P2 Foundation B: the migration back-compat harness — proof that a NEW
//! migration preserves a DEPLOYED database's existing rows (the net the
//! fresh-state tests miss). Applies the REAL migration `.sql` files from disk via
//! raw SQL, seeds rows, then applies the migration under test and asserts the
//! rows survive. Names match `specs/store/p7-migration-backcompat.spec.md`.

use std::path::PathBuf;

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

// NOTE (design-faithful C1 redirect): the `0002 runs.worktree_path` migration was
// REVERTED — the design's per-task_run worktree lives on the existing
// `task_runs.worktree_path` (nullable in 0001), so no new column is needed for
// the worktree. The harness below STANDS (model-agnostic, reusable). P108 now
// uses it for C2's `review_runs` round migration; the self-test still proves
// the harness is not vacuous.

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

#[tokio::test]
async fn review_runs_round_migration_preserves_existing_rows() {
    let pool = raw_pool().await;
    apply(&pool, "0001_init.sql").await;
    sqlx::query(
        "INSERT INTO runs (id, workflow_sha, status, started_at, last_heartbeat) \
         VALUES ('r1','sha','running',1,1)",
    )
    .execute(&pool)
    .await
    .expect("seed run");
    sqlx::query(
        "INSERT INTO review_runs \
         (id, run_id, node_id, expected, context_sha, started_at) \
         VALUES ('rr1','r1','review',3,'csha',1)",
    )
    .execute(&pool)
    .await
    .expect("seed old review run");

    apply(&pool, "0002_review_runs_round.sql").await;

    let row = sqlx::query("SELECT round FROM review_runs WHERE id = 'rr1'")
        .fetch_one(&pool)
        .await
        .expect("review run survived");
    let round: i64 = row.get("round");
    assert_eq!(round, 1, "pre-migration review runs default to round 1");
}
