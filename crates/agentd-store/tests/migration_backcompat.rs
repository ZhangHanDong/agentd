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
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
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

#[tokio::test]
async fn agent_registry_lifecycle_migration_preserves_existing_agents() {
    let pool = raw_pool().await;
    apply(&pool, "0001_init.sql").await;
    apply(&pool, "0002_review_runs_round.sql").await;
    sqlx::query(
        "INSERT INTO agents \
         (id, mxid, role, backend, backend_target, prompt_profile, enabled, created_at) \
         VALUES ('legacy-reviewer','@legacy:agentd.local','reviewer','tmux','legacy:0.0','default',1,10)",
    )
    .execute(&pool)
    .await
    .expect("seed legacy agent");

    apply(&pool, "0003_agent_registry_lifecycle.sql").await;

    let row = sqlx::query(
        "SELECT id, name, role, backend, status, registered_at, updated_at \
         FROM agents WHERE id = 'legacy-reviewer'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy agent survived");
    assert_eq!(row.get::<String, _>("name"), "legacy-reviewer");
    assert_eq!(row.get::<String, _>("role"), "reviewer");
    assert_eq!(row.get::<String, _>("backend"), "tmux");
    assert_eq!(row.get::<String, _>("status"), "offline");
    assert_eq!(row.get::<i64, _>("registered_at"), 10);
    assert_eq!(row.get::<i64, _>("updated_at"), 10);
}

#[tokio::test]
async fn agent_runtime_lifecycle_migration_preserves_existing_agents() {
    let pool = raw_pool().await;
    apply(&pool, "0001_init.sql").await;
    apply(&pool, "0002_review_runs_round.sql").await;
    apply(&pool, "0003_agent_registry_lifecycle.sql").await;
    sqlx::query(
        "INSERT INTO agents \
         (id, mxid, role, backend, enabled, created_at, name, runtime, workdir, status, \
          registered_at, updated_at, runtime_profile) \
         VALUES ('codex-worker','@codex:agentd.local','agent','tmux',1,10,'codex-worker',\
          'codex','/tmp/agentd/codex-worker','offline',10,10,'{}')",
    )
    .execute(&pool)
    .await
    .expect("seed p213 agent");

    apply(&pool, "0004_agent_runtime_lifecycle.sql").await;

    let row = sqlx::query("SELECT id, name, runtime_state FROM agents WHERE id = 'codex-worker'")
        .fetch_one(&pool)
        .await
        .expect("agent survived");
    assert_eq!(row.get::<String, _>("name"), "codex-worker");
    assert_eq!(
        row.get::<String, _>("runtime_state"),
        "{}",
        "existing agents default to empty runtime state"
    );
}

async fn base_row_fingerprint(pool: &SqlitePool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT 'project|' || id || '|' || name || '|' || repo_path FROM projects \
         UNION ALL SELECT 'agent|' || id || '|' || name || '|' || status FROM agents \
         UNION ALL SELECT 'issue|' || id || '|' || project_id || '|' || title FROM issues \
         UNION ALL SELECT 'run|' || id || '|' || project_id || '|' || status FROM runs \
         UNION ALL SELECT 'task|' || id || '|' || run_id || '|' || status FROM task_runs \
         UNION ALL SELECT 'message|' || id || '|' || from_agent || '|' || to_agent FROM direct_messages \
         UNION ALL SELECT 'queue|' || ticket || '|' || role || '|' || status FROM agent_scheduler_queue \
         UNION ALL SELECT 'matrix|' || room_id || '|' || trusted || '|' || trust_reason FROM matrix_bridge_rooms \
         ORDER BY 1",
    )
    .fetch_all(pool)
    .await
    .expect("base row fingerprint")
}

#[tokio::test]
async fn enterprise_identity_migration_preserves_base_rows() {
    let pool = raw_pool().await;
    for migration in [
        "0001_init.sql",
        "0002_review_runs_round.sql",
        "0003_agent_registry_lifecycle.sql",
        "0004_agent_runtime_lifecycle.sql",
        "0005_direct_messages.sql",
        "0006_group_messages.sql",
        "0007_message_attachments.sql",
        "0008_agent_chat_task_import.sql",
        "0009_direct_message_schema_and_live_task_graph.sql",
        "0010_agent_scheduler.sql",
        "0011_remote_relay_backend.sql",
        "0012_matrix_bridge_contract.sql",
    ] {
        apply(&pool, migration).await;
    }

    sqlx::raw_sql(
        "INSERT INTO projects (id, name, repo_path, mempal_wing, created_at, updated_at) \
         VALUES ('project-base', 'Base', '/tmp/base', 'wing-base', 1, 1); \
         INSERT INTO agents \
         (id, mxid, role, backend, enabled, created_at, name, status, registered_at, updated_at) \
         VALUES ('agent-base', '@agent-base:local', 'coding', 'tmux', 1, 1, 'agent-base', 'online', 1, 1); \
         INSERT INTO issues \
         (id, project_id, title, body, labels, state, fetched_at) \
         VALUES ('issue-base', 'project-base', 'Issue', 'Body', '[]', 'open', 1); \
         INSERT INTO runs \
         (id, project_id, workflow_sha, status, started_at, last_heartbeat) \
         VALUES ('r_base', 'project-base', 'sha', 'running', 1, 1); \
         INSERT INTO task_runs (id, run_id, node_id, status, started_at) \
         VALUES ('tr_base', 'r_base', 'impl', 'running', 1); \
         INSERT INTO direct_messages \
         (id, ts, from_agent, to_agent, message_type, priority, summary, full, source, created_at) \
         VALUES ('msg-base', 1, 'agent-base', 'operator', 'inform', 'normal', 'summary', 'full', 'api', 1); \
         INSERT INTO agent_scheduler_queue \
         (ticket, role, tier, status, created_at, updated_at) \
         VALUES ('disp-base', 'coding', 'strong', 'queued', 1, 1); \
         INSERT INTO matrix_bridge_rooms \
         (room_id, trusted, trust_reason, created_at, updated_at) \
         VALUES ('!base:local', 1, 'seed', 1, 1);",
    )
    .execute(&pool)
    .await
    .expect("seed base rows");

    let before = base_row_fingerprint(&pool).await;
    apply(&pool, "0013_enterprise_agent_worker_runtime.sql").await;
    let after = base_row_fingerprint(&pool).await;
    assert_eq!(after, before);

    for table in [
        "agent_profiles",
        "legacy_agent_aliases",
        "workers",
        "worker_incarnations",
        "runtime_sessions",
        "runtime_attempts",
    ] {
        let count: i64 =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
                .fetch_one(&pool)
                .await
                .expect("enterprise row count");
        assert_eq!(count, 0, "{table} must start empty");
    }
    let version: String = sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = 'version'")
        .fetch_one(&pool)
        .await
        .expect("schema version");
    assert_eq!(version, "13");
}

#[tokio::test]
async fn enterprise_artifact_audit_migration_preserves_existing_rows() {
    let pool = raw_pool().await;
    for migration in [
        "0001_init.sql",
        "0002_review_runs_round.sql",
        "0003_agent_registry_lifecycle.sql",
        "0004_agent_runtime_lifecycle.sql",
        "0005_direct_messages.sql",
        "0006_group_messages.sql",
        "0007_message_attachments.sql",
        "0008_agent_chat_task_import.sql",
        "0009_direct_message_schema_and_live_task_graph.sql",
        "0010_agent_scheduler.sql",
        "0011_remote_relay_backend.sql",
        "0012_matrix_bridge_contract.sql",
        "0013_enterprise_agent_worker_runtime.sql",
    ] {
        apply(&pool, migration).await;
    }
    let sha = "a".repeat(64);
    sqlx::query(
        "INSERT INTO runs (id, workflow_sha, status, started_at, last_heartbeat) \
         VALUES ('r_p268_legacy', 'workflow-sha', 'running', 10, 11)",
    )
    .execute(&pool)
    .await
    .expect("seed legacy run");
    sqlx::query(
        "INSERT INTO artifacts (sha256, kind, path, bytes, created_at, run_id, node_id) \
         VALUES (?, 'transcript', '/tmp/transcript.log', 42, 12, 'r_p268_legacy', 'impl')",
    )
    .bind(&sha)
    .execute(&pool)
    .await
    .expect("seed legacy artifact");
    sqlx::query(
        "INSERT INTO events (project_id, run_id, kind, payload, emitted_at) \
         VALUES (NULL, 'r_p268_legacy', 'run.output', '{\"bytes\":42}', 13)",
    )
    .execute(&pool)
    .await
    .expect("seed legacy event");

    let artifact_before: (String, String, i64, i64, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT kind, path, bytes, created_at, run_id, node_id FROM artifacts WHERE sha256 = ?",
        )
        .bind(&sha)
        .fetch_one(&pool)
        .await
        .expect("legacy artifact before");
    let event_before: (i64, Option<String>, String, String, i64) = sqlx::query_as(
        "SELECT seq, run_id, kind, payload, emitted_at FROM events WHERE run_id='r_p268_legacy'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy event before");

    apply(&pool, "0014_enterprise_artifact_audit.sql").await;

    let artifact_after: (String, String, i64, i64, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT kind, path, bytes, created_at, run_id, node_id FROM artifacts WHERE sha256 = ?",
        )
        .bind(&sha)
        .fetch_one(&pool)
        .await
        .expect("legacy artifact after");
    let event_after: (i64, Option<String>, String, String, i64) = sqlx::query_as(
        "SELECT seq, run_id, kind, payload, emitted_at FROM events WHERE run_id='r_p268_legacy'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy event after");
    assert_eq!(artifact_after, artifact_before);
    assert_eq!(event_after, event_before);

    for table in [
        "execution_artifacts",
        "legacy_artifact_mappings",
        "artifact_certification_refs",
        "execution_audit_events",
    ] {
        let count: i64 =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|err| panic!("count {table}: {err}"));
        assert_eq!(count, 0, "{table} must start empty");
    }
    let version: String = sqlx::query_scalar("SELECT value FROM schema_meta WHERE key='version'")
        .fetch_one(&pool)
        .await
        .expect("schema version");
    assert_eq!(version, "14");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_lease_migration_preserves_enterprise_and_compatibility_rows() {
    let pool = raw_pool().await;
    for migration in [
        "0001_init.sql",
        "0002_review_runs_round.sql",
        "0003_agent_registry_lifecycle.sql",
        "0004_agent_runtime_lifecycle.sql",
        "0005_direct_messages.sql",
        "0006_group_messages.sql",
        "0007_message_attachments.sql",
        "0008_agent_chat_task_import.sql",
        "0009_direct_message_schema_and_live_task_graph.sql",
        "0010_agent_scheduler.sql",
        "0011_remote_relay_backend.sql",
        "0012_matrix_bridge_contract.sql",
        "0013_enterprise_agent_worker_runtime.sql",
        "0014_enterprise_artifact_audit.sql",
    ] {
        apply(&pool, migration).await;
    }

    let hash = "a".repeat(64);
    sqlx::raw_sql(
        "INSERT INTO runs (id, workflow_sha, status, started_at, last_heartbeat) \
         VALUES ('r_01ARZ3NDEKTSV4RRFFQ69G5FAV', 'sha', 'running', 1, 1); \
         INSERT INTO task_runs (id, run_id, node_id, status, started_at) \
         VALUES ('tr_01ARZ3NDEKTSV4RRFFQ69G5FAW', 'r_01ARZ3NDEKTSV4RRFFQ69G5FAV', 'impl', 'running', 1); \
         INSERT INTO workers (id, status, trust_domain, created_at, updated_at) \
         VALUES ('wk_01ARZ3NDEKTSV4RRFFQ69G5FAX', 'online', 'test', 1, 1); \
         INSERT INTO worker_incarnations \
         (id, worker_id, daemon_version, host_name, capabilities_json, is_current, registered_at, last_seen_at) \
         VALUES ('wi_01ARZ3NDEKTSV4RRFFQ69G5FAY', 'wk_01ARZ3NDEKTSV4RRFFQ69G5FAX', \
                 'p270', 'host', '{}', 1, 1, 1); \
         INSERT INTO agent_scheduler_reservations \
         (id, role, tier, status, ticket, created_at, updated_at) \
         VALUES ('reservation-p270', 'coding', 'strong', 'reserved', 'ticket-p270', 1, 1); \
         INSERT INTO agent_scheduler_queue \
         (ticket, role, tier, status, created_at, updated_at, reservation_id) \
         VALUES ('ticket-p270', 'coding', 'strong', 'queued', 1, 1, 'reservation-p270');",
    )
    .execute(&pool)
    .await
    .expect("seed existing execution and scheduler rows");
    sqlx::query(
        "INSERT INTO execution_artifacts \
         (id, kind, content_sha256, size_bytes, media_type, storage_ref, provenance_json, \
          execution_run_id, execution_task_id, producer_worker_incarnation_id, \
          snapshot_authority_key, snapshot_resource_kind, snapshot_resource_id, \
          snapshot_resource_version, snapshot_content_sha256, target_repository_id, \
          target_base_commit, created_at) \
         VALUES ('ar_01ARZ3NDEKTSV4RRFFQ69G5FAZ', 'log', ?, 1, 'text/plain', \
                 'cas://p270', '{}', 'r_01ARZ3NDEKTSV4RRFFQ69G5FAV', \
                 'tr_01ARZ3NDEKTSV4RRFFQ69G5FAW', 'wi_01ARZ3NDEKTSV4RRFFQ69G5FAY', \
                 'specify:corp', 'execution_snapshot', 'snapshot-1', '1', ?, \
                 'repo-1', 'base-1', 2)",
    )
    .bind(&hash)
    .bind(&hash)
    .execute(&pool)
    .await
    .expect("seed artifact");
    sqlx::query(
        "INSERT INTO execution_audit_events \
         (id, idempotency_scope, idempotency_key, event_type, actor_kind, actor_ref, \
          payload_sha256, payload_json, execution_run_id, execution_task_id, \
          worker_incarnation_id, snapshot_authority_key, snapshot_resource_kind, \
          snapshot_resource_id, snapshot_resource_version, snapshot_content_sha256, \
          target_repository_id, target_base_commit, occurred_at, recorded_at) \
         VALUES ('ae_01ARZ3NDEKTSV4RRFFQ69G5FB0', 'p270', 'seed', 'task.seeded', \
                 'control_plane', 'agentd', ?, '{}', 'r_01ARZ3NDEKTSV4RRFFQ69G5FAV', \
                 'tr_01ARZ3NDEKTSV4RRFFQ69G5FAW', 'wi_01ARZ3NDEKTSV4RRFFQ69G5FAY', \
                 'specify:corp', 'execution_snapshot', 'snapshot-1', '1', ?, \
                 'repo-1', 'base-1', 2, 2)",
    )
    .bind(&hash)
    .bind(&hash)
    .execute(&pool)
    .await
    .expect("seed audit");

    let before: Vec<String> = sqlx::query_scalar(
        "SELECT 'task|' || id || '|' || run_id || '|' || status FROM task_runs \
         UNION ALL SELECT 'worker|' || id || '|' || status || '|' || trust_domain FROM workers \
         UNION ALL SELECT 'incarnation|' || id || '|' || worker_id || '|' || is_current FROM worker_incarnations \
         UNION ALL SELECT 'reservation|' || id || '|' || ticket || '|' || status FROM agent_scheduler_reservations \
         UNION ALL SELECT 'queue|' || ticket || '|' || reservation_id || '|' || status FROM agent_scheduler_queue \
         UNION ALL SELECT 'artifact|' || id || '|' || execution_task_id || '|' || storage_ref FROM execution_artifacts \
         UNION ALL SELECT 'audit|' || id || '|' || execution_task_id || '|' || event_type FROM execution_audit_events \
         ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .expect("fingerprint before P270");

    apply(&pool, "0015_enterprise_task_leases.sql").await;

    let after: Vec<String> = sqlx::query_scalar(
        "SELECT 'task|' || id || '|' || run_id || '|' || status FROM task_runs \
         UNION ALL SELECT 'worker|' || id || '|' || status || '|' || trust_domain FROM workers \
         UNION ALL SELECT 'incarnation|' || id || '|' || worker_id || '|' || is_current FROM worker_incarnations \
         UNION ALL SELECT 'reservation|' || id || '|' || ticket || '|' || status FROM agent_scheduler_reservations \
         UNION ALL SELECT 'queue|' || ticket || '|' || reservation_id || '|' || status FROM agent_scheduler_queue \
         UNION ALL SELECT 'artifact|' || id || '|' || execution_task_id || '|' || storage_ref FROM execution_artifacts \
         UNION ALL SELECT 'audit|' || id || '|' || execution_task_id || '|' || event_type FROM execution_audit_events \
         ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .expect("fingerprint after P270");
    assert_eq!(after, before);

    for table in ["execution_task_leases", "execution_task_lease_heads"] {
        let count: i64 =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
                .fetch_one(&pool)
                .await
                .expect("P270 table count");
        assert_eq!(count, 0, "{table} starts empty");
    }
    let ticket_promotions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_task_leases \
         WHERE id IN ('ticket-p270', 'reservation-p270')",
    )
    .fetch_one(&pool)
    .await
    .expect("no compatibility identity promotion");
    assert_eq!(ticket_promotions, 0);

    let version: String = sqlx::query_scalar("SELECT value FROM schema_meta WHERE key='version'")
        .fetch_one(&pool)
        .await
        .expect("schema version");
    assert_eq!(version, "15");
}
