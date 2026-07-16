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
        "direct_messages",
        "groups",
        "group_members",
        "group_messages",
        "group_message_reads",
        "group_mention_reads",
        "agent_chat_tasks",
        "agent_chat_task_graphs",
        "agent_scheduler_reservations",
        "agent_scheduler_queue",
        "relay_servers",
        "delivery_events",
        "relay_stream_events",
        "matrix_bridge_rooms",
        "matrix_bridge_events",
        "execution_task_leases",
        "execution_task_lease_heads",
        "enterprise_fleet_queue",
        "enterprise_worker_availability",
        "enterprise_scheduler_outbox",
        "enterprise_scheduler_report_receipts",
        "enterprise_artifact_upload_acknowledgements",
        "enterprise_side_effect_admissions",
        "enterprise_fencing_rejections",
        "matrix_gateway_project_bindings",
        "matrix_gateway_commands",
        "matrix_gateway_inbox",
        "matrix_gateway_outbox",
        "matrix_gateway_cutover_history",
        "matrix_gateway_state_mappings",
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
    assert_eq!(version, "20");
}

#[tokio::test]
async fn migration_creates_group_message_tables() {
    let (store, _dir) = open_temp().await;
    let rows = sqlx::query("SELECT name FROM sqlite_master WHERE type='table'")
        .fetch_all(store.pool())
        .await
        .expect("query tables");
    let tables: Vec<String> = rows.iter().map(|r| r.get::<String, _>("name")).collect();
    for expected in [
        "groups",
        "group_members",
        "group_messages",
        "group_message_reads",
        "group_mention_reads",
    ] {
        assert!(
            tables.contains(&expected.to_string()),
            "missing table '{expected}'; got {tables:?}"
        );
    }

    let version: String = sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = 'version'")
        .fetch_one(store.pool())
        .await
        .expect("schema version row");
    let parsed: i64 = version.parse().expect("integer schema version");
    assert!(
        parsed >= 6,
        "group table migration must have applied; got version {version}"
    );
}

#[tokio::test]
async fn migration_adds_message_attachment_columns() {
    let (store, _dir) = open_temp().await;
    for (table, column) in [
        ("direct_messages", "attachments_json"),
        ("group_messages", "attachments_json"),
    ] {
        let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
            .fetch_all(store.pool())
            .await
            .expect("message table columns");
        let columns: Vec<String> = rows.iter().map(|r| r.get::<String, _>("name")).collect();
        assert!(
            columns.contains(&column.to_string()),
            "missing {table}.{column}; got {columns:?}"
        );
    }

    let version: String = sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = 'version'")
        .fetch_one(store.pool())
        .await
        .expect("schema version row");
    assert_eq!(version, "19");
}

#[tokio::test]
async fn migration_creates_remote_relay_backend_tables() {
    let (store, _dir) = open_temp().await;
    let rows = sqlx::query("SELECT name FROM sqlite_master WHERE type='table'")
        .fetch_all(store.pool())
        .await
        .expect("query tables");
    let tables: Vec<String> = rows.iter().map(|r| r.get::<String, _>("name")).collect();
    for expected in ["relay_servers", "delivery_events", "relay_stream_events"] {
        assert!(
            tables.contains(&expected.to_string()),
            "missing table '{expected}'; got {tables:?}"
        );
    }

    let relay_columns = sqlx::query("PRAGMA table_info(relay_servers)")
        .fetch_all(store.pool())
        .await
        .expect("relay server table columns");
    let relay_columns: Vec<String> = relay_columns
        .iter()
        .map(|r| r.get::<String, _>("name"))
        .collect();
    for expected in [
        "id",
        "instance_id",
        "boot_ts",
        "agents_json",
        "sessions_json",
        "agent_count",
        "online",
        "maintenance",
        "last_seen_at",
        "heartbeat_at",
        "updated_at",
    ] {
        assert!(
            relay_columns.contains(&expected.to_string()),
            "missing relay_servers.{expected}; got {relay_columns:?}"
        );
    }

    let delivery_columns = sqlx::query("PRAGMA table_info(delivery_events)")
        .fetch_all(store.pool())
        .await
        .expect("delivery event table columns");
    let delivery_columns: Vec<String> = delivery_columns
        .iter()
        .map(|r| r.get::<String, _>("name"))
        .collect();
    for expected in [
        "id",
        "seq",
        "type",
        "message_id",
        "queue_entry_id",
        "agent",
        "target",
        "reason",
        "source",
        "context_json",
        "created_at",
    ] {
        assert!(
            delivery_columns.contains(&expected.to_string()),
            "missing delivery_events.{expected}; got {delivery_columns:?}"
        );
    }

    let stream_columns = sqlx::query("PRAGMA table_info(relay_stream_events)")
        .fetch_all(store.pool())
        .await
        .expect("relay stream event table columns");
    let stream_columns: Vec<String> = stream_columns
        .iter()
        .map(|r| r.get::<String, _>("name"))
        .collect();
    for expected in ["seq", "event", "payload_json", "created_at"] {
        assert!(
            stream_columns.contains(&expected.to_string()),
            "missing relay_stream_events.{expected}; got {stream_columns:?}"
        );
    }

    let version: String = sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = 'version'")
        .fetch_one(store.pool())
        .await
        .expect("schema version row");
    assert_eq!(version, "19");
}

#[tokio::test]
async fn migration_creates_matrix_bridge_contract_tables() {
    let (store, _dir) = open_temp().await;
    let rows = sqlx::query("SELECT name FROM sqlite_master WHERE type='table'")
        .fetch_all(store.pool())
        .await
        .expect("query tables");
    let tables: Vec<String> = rows.iter().map(|r| r.get::<String, _>("name")).collect();
    for expected in ["matrix_bridge_rooms", "matrix_bridge_events"] {
        assert!(
            tables.contains(&expected.to_string()),
            "missing table '{expected}'; got {tables:?}"
        );
    }

    let room_columns = sqlx::query("PRAGMA table_info(matrix_bridge_rooms)")
        .fetch_all(store.pool())
        .await
        .expect("matrix bridge room table columns");
    let room_columns: Vec<String> = room_columns
        .iter()
        .map(|r| r.get::<String, _>("name"))
        .collect();
    for expected in [
        "room_id",
        "group_name",
        "agent_name",
        "trusted",
        "trust_reason",
        "inviter_mxid",
        "created_at",
        "updated_at",
    ] {
        assert!(
            room_columns.contains(&expected.to_string()),
            "missing matrix_bridge_rooms.{expected}; got {room_columns:?}"
        );
    }

    let event_columns = sqlx::query("PRAGMA table_info(matrix_bridge_events)")
        .fetch_all(store.pool())
        .await
        .expect("matrix bridge event table columns");
    let event_columns: Vec<String> = event_columns
        .iter()
        .map(|r| r.get::<String, _>("name"))
        .collect();
    for expected in [
        "event_id",
        "room_id",
        "sender_mxid",
        "message_id",
        "route",
        "ignored",
        "created_at",
    ] {
        assert!(
            event_columns.contains(&expected.to_string()),
            "missing matrix_bridge_events.{expected}; got {event_columns:?}"
        );
    }

    let version: String = sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = 'version'")
        .fetch_one(store.pool())
        .await
        .expect("schema version row");
    assert_eq!(version, "19");
}

#[tokio::test]
async fn migration_creates_agent_chat_task_import_tables() {
    let (store, _dir) = open_temp().await;
    let rows = sqlx::query("SELECT name FROM sqlite_master WHERE type='table'")
        .fetch_all(store.pool())
        .await
        .expect("query tables");
    let tables: Vec<String> = rows.iter().map(|r| r.get::<String, _>("name")).collect();
    for expected in ["agent_chat_tasks", "agent_chat_task_graphs"] {
        assert!(
            tables.contains(&expected.to_string()),
            "missing table '{expected}'; got {tables:?}"
        );
    }

    let task_columns = sqlx::query("PRAGMA table_info(agent_chat_tasks)")
        .fetch_all(store.pool())
        .await
        .expect("task import table columns");
    let task_columns: Vec<String> = task_columns
        .iter()
        .map(|r| r.get::<String, _>("name"))
        .collect();
    for expected in ["id", "status", "assignee", "raw_json", "imported_at"] {
        assert!(
            task_columns.contains(&expected.to_string()),
            "missing agent_chat_tasks.{expected}; got {task_columns:?}"
        );
    }

    let graph_columns = sqlx::query("PRAGMA table_info(agent_chat_task_graphs)")
        .fetch_all(store.pool())
        .await
        .expect("task graph import table columns");
    let graph_columns: Vec<String> = graph_columns
        .iter()
        .map(|r| r.get::<String, _>("name"))
        .collect();
    for expected in ["id", "raw_json", "imported_at"] {
        assert!(
            graph_columns.contains(&expected.to_string()),
            "missing agent_chat_task_graphs.{expected}; got {graph_columns:?}"
        );
    }

    let version: String = sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = 'version'")
        .fetch_one(store.pool())
        .await
        .expect("schema version row");
    assert_eq!(version, "19");
}

#[tokio::test]
async fn migration_adds_direct_message_schema_column() {
    let (store, _dir) = open_temp().await;
    let rows = sqlx::query("PRAGMA table_info(direct_messages)")
        .fetch_all(store.pool())
        .await
        .expect("direct message columns");
    let columns: Vec<String> = rows.iter().map(|r| r.get::<String, _>("name")).collect();
    assert!(
        columns.contains(&"schema_json".to_string()),
        "missing direct_messages.schema_json; got {columns:?}"
    );

    let version: String = sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = 'version'")
        .fetch_one(store.pool())
        .await
        .expect("schema version row");
    assert_eq!(version, "19");
}

#[tokio::test]
async fn migration_adds_agent_registry_lifecycle_columns() {
    let (store, _dir) = open_temp().await;
    let rows = sqlx::query("PRAGMA table_info(agents)")
        .fetch_all(store.pool())
        .await
        .expect("agent columns");
    let columns: Vec<String> = rows.iter().map(|r| r.get::<String, _>("name")).collect();
    for expected in [
        "name",
        "capability",
        "runtime",
        "model",
        "tmux_target",
        "home_dir",
        "workdir",
        "state_dir",
        "server",
        "status",
        "offline_reason",
        "last_seen_at",
        "registered_at",
        "updated_at",
        "runtime_profile",
        "runtime_state",
    ] {
        assert!(
            columns.contains(&expected.to_string()),
            "missing agents.{expected}; got {columns:?}"
        );
    }
}

#[tokio::test]
async fn migration_creates_agent_scheduler_tables() {
    let (store, _dir) = open_temp().await;
    let rows = sqlx::query("SELECT name FROM sqlite_master WHERE type='table'")
        .fetch_all(store.pool())
        .await
        .expect("query tables");
    let tables: Vec<String> = rows.iter().map(|r| r.get::<String, _>("name")).collect();
    for expected in ["agent_scheduler_reservations", "agent_scheduler_queue"] {
        assert!(
            tables.contains(&expected.to_string()),
            "missing table '{expected}'; got {tables:?}"
        );
    }

    let reservation_columns = sqlx::query("PRAGMA table_info(agent_scheduler_reservations)")
        .fetch_all(store.pool())
        .await
        .expect("reservation table columns");
    let reservation_columns: Vec<String> = reservation_columns
        .iter()
        .map(|r| r.get::<String, _>("name"))
        .collect();
    for expected in [
        "id",
        "role",
        "tier",
        "agent",
        "provisioned_name",
        "status",
        "task_json",
        "room",
        "runtime_json",
        "ticket",
        "created_at",
        "updated_at",
        "released_at",
    ] {
        assert!(
            reservation_columns.contains(&expected.to_string()),
            "missing agent_scheduler_reservations.{expected}; got {reservation_columns:?}"
        );
    }

    let queue_columns = sqlx::query("PRAGMA table_info(agent_scheduler_queue)")
        .fetch_all(store.pool())
        .await
        .expect("scheduler queue table columns");
    let queue_columns: Vec<String> = queue_columns
        .iter()
        .map(|r| r.get::<String, _>("name"))
        .collect();
    for expected in [
        "ticket",
        "role",
        "tier",
        "task_json",
        "room",
        "status",
        "created_at",
        "updated_at",
        "drained_at",
        "reservation_id",
    ] {
        assert!(
            queue_columns.contains(&expected.to_string()),
            "missing agent_scheduler_queue.{expected}; got {queue_columns:?}"
        );
    }

    let version: String = sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = 'version'")
        .fetch_one(store.pool())
        .await
        .expect("schema version row");
    assert_eq!(version, "19");
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

#[tokio::test]
async fn migration_creates_enterprise_agent_worker_runtime_tables() {
    let (store, _dir) = open_temp().await;
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .fetch_all(store.pool())
            .await
            .expect("enterprise tables");
    for expected in [
        "agent_profiles",
        "legacy_agent_aliases",
        "workers",
        "worker_incarnations",
        "runtime_sessions",
        "runtime_attempts",
    ] {
        assert!(
            tables.iter().any(|table| table == expected),
            "missing {expected}"
        );
    }

    let version: String = sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = 'version'")
        .fetch_one(store.pool())
        .await
        .expect("schema version");
    assert_eq!(version, "19");

    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'index' AND name LIKE 'idx_%one_current'",
    )
    .fetch_all(store.pool())
    .await
    .expect("enterprise indexes");
    assert!(indexes.contains(&"idx_worker_incarnations_one_current".to_string()));
    assert!(indexes.contains(&"idx_runtime_attempts_one_current".to_string()));

    let profile_fk_tables: Vec<String> =
        sqlx::query_scalar("SELECT \"table\" FROM pragma_foreign_key_list('legacy_agent_aliases')")
            .fetch_all(store.pool())
            .await
            .expect("alias foreign keys");
    assert!(profile_fk_tables.contains(&"agents".to_string()));
    assert!(profile_fk_tables.contains(&"agent_profiles".to_string()));

    let invalid_id = sqlx::query(
        "INSERT INTO agent_profiles \
         (id, role, runtime, status, created_at, updated_at) \
         VALUES ('legacy-profile', 'coding', 'codex', 'active', 1, 1)",
    )
    .execute(store.pool())
    .await;
    assert!(invalid_id.is_err(), "invalid canonical id must fail");

    let invalid_status = sqlx::query(
        "INSERT INTO agent_profiles \
         (id, role, runtime, status, created_at, updated_at) \
         VALUES ('ap_01ARZ3NDEKTSV4RRFFQ69G5FAV', 'coding', 'codex', 'online', 1, 1)",
    )
    .execute(store.pool())
    .await;
    assert!(invalid_status.is_err(), "invalid profile status must fail");
}

#[tokio::test]
async fn migration_creates_enterprise_artifact_audit_tables() {
    let (store, _dir) = open_temp().await;
    let tables: Vec<String> = sqlx::query("SELECT name FROM sqlite_master WHERE type='table'")
        .fetch_all(store.pool())
        .await
        .expect("query tables")
        .iter()
        .map(|row| row.get::<String, _>("name"))
        .collect();
    for expected in [
        "execution_artifacts",
        "legacy_artifact_mappings",
        "artifact_certification_refs",
        "execution_audit_events",
    ] {
        assert!(
            tables.contains(&expected.to_string()),
            "missing P268 table {expected}; got {tables:?}"
        );
    }

    let version: String = sqlx::query_scalar("SELECT value FROM schema_meta WHERE key='version'")
        .fetch_one(store.pool())
        .await
        .expect("schema version");
    assert_eq!(version, "19");

    let triggers: Vec<String> =
        sqlx::query("SELECT name FROM sqlite_master WHERE type='trigger' ORDER BY name")
            .fetch_all(store.pool())
            .await
            .expect("query triggers")
            .iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();
    for expected in [
        "trg_execution_artifacts_no_update",
        "trg_execution_artifacts_no_delete",
        "trg_artifact_certification_refs_no_update",
        "trg_artifact_certification_refs_no_delete",
        "trg_execution_audit_events_no_update",
        "trg_execution_audit_events_no_delete",
    ] {
        assert!(
            triggers.contains(&expected.to_string()),
            "missing immutability trigger {expected}; got {triggers:?}"
        );
    }

    sqlx::query(
        "INSERT INTO runs (id, workflow_sha, status, started_at, last_heartbeat) \
         VALUES ('r_p268_schema', 'sha', 'running', 1, 1)",
    )
    .execute(store.pool())
    .await
    .expect("seed run");
    let invalid_artifact = sqlx::query(
        "INSERT INTO execution_artifacts \
         (id, kind, content_sha256, size_bytes, media_type, storage_ref, provenance_json, \
          execution_run_id, snapshot_authority_key, snapshot_resource_kind, \
          snapshot_resource_id, snapshot_resource_version, snapshot_content_sha256, \
          target_repository_id, target_base_commit, created_at) \
         VALUES ('legacy', 'spec', 'bad', 1, 'text/plain', 'cas://bad', '{bad', \
                 'r_p268_schema', 'specify', 'execution_snapshot', 'snap', '1', 'bad', \
                 'repo', 'base', 1)",
    )
    .execute(store.pool())
    .await;
    assert!(
        invalid_artifact.is_err(),
        "canonical id/hash/JSON checks must reject direct invalid insert"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn migration_creates_constrained_task_lease_tables() {
    let (store, _dir) = open_temp().await;
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .fetch_all(store.pool())
            .await
            .expect("lease tables");
    for expected in ["execution_task_leases", "execution_task_lease_heads"] {
        assert!(
            tables.iter().any(|table| table == expected),
            "missing P270 table {expected}"
        );
    }

    let version: String = sqlx::query_scalar("SELECT value FROM schema_meta WHERE key='version'")
        .fetch_one(store.pool())
        .await
        .expect("schema version");
    assert_eq!(version, "19");

    let lease_fk_tables: Vec<String> = sqlx::query_scalar(
        "SELECT \"table\" FROM pragma_foreign_key_list('execution_task_leases') ORDER BY \"table\"",
    )
    .fetch_all(store.pool())
    .await
    .expect("lease foreign keys");
    assert_eq!(
        lease_fk_tables,
        vec!["task_runs".to_string(), "worker_incarnations".to_string()]
    );

    let indexes: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='index' ORDER BY name")
            .fetch_all(store.pool())
            .await
            .expect("lease indexes");
    for expected in [
        "idx_execution_task_leases_one_active",
        "idx_execution_task_leases_task_token",
        "idx_execution_task_leases_worker_status",
    ] {
        assert!(
            indexes.contains(&expected.to_string()),
            "missing {expected}"
        );
    }

    let triggers: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='trigger' ORDER BY name")
            .fetch_all(store.pool())
            .await
            .expect("lease triggers");
    for expected in [
        "trg_execution_task_leases_identity_immutable",
        "trg_execution_task_leases_terminal_immutable",
        "trg_execution_task_leases_no_delete",
        "trg_execution_task_lease_heads_token_monotonic",
        "trg_execution_task_lease_heads_current_valid",
        "trg_execution_task_lease_heads_no_delete",
    ] {
        assert!(
            triggers.contains(&expected.to_string()),
            "missing {expected}"
        );
    }

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
                 'p270', 'host', '{}', 1, 1, 1);",
    )
    .execute(store.pool())
    .await
    .expect("lease parents");

    let invalid_id = sqlx::query(
        "INSERT INTO execution_task_leases \
         (id, execution_task_id, worker_incarnation_id, fencing_token, status, \
          acquired_at, expires_at, record_version) \
         VALUES ('ticket-1', 'tr_01ARZ3NDEKTSV4RRFFQ69G5FAW', \
                 'wi_01ARZ3NDEKTSV4RRFFQ69G5FAY', 1, 'active', 1, 2, 1)",
    )
    .execute(store.pool())
    .await;
    assert!(invalid_id.is_err(), "noncanonical lease id must fail");

    let invalid_token = sqlx::query(
        "INSERT INTO execution_task_leases \
         (id, execution_task_id, worker_incarnation_id, fencing_token, status, \
          acquired_at, expires_at, record_version) \
         VALUES ('ls_01ARZ3NDEKTSV4RRFFQ69G5FAZ', 'tr_01ARZ3NDEKTSV4RRFFQ69G5FAW', \
                 'wi_01ARZ3NDEKTSV4RRFFQ69G5FAY', 0, 'active', 1, 2, 1)",
    )
    .execute(store.pool())
    .await;
    assert!(invalid_token.is_err(), "zero fencing token must fail");

    sqlx::query(
        "INSERT INTO execution_task_leases \
         (id, execution_task_id, worker_incarnation_id, fencing_token, status, \
          acquired_at, expires_at, record_version) \
         VALUES ('ls_01ARZ3NDEKTSV4RRFFQ69G5FB0', 'tr_01ARZ3NDEKTSV4RRFFQ69G5FAW', \
                 'wi_01ARZ3NDEKTSV4RRFFQ69G5FAY', 1, 'active', 1, 2, 1)",
    )
    .execute(store.pool())
    .await
    .expect("valid active lease");
    sqlx::query(
        "INSERT INTO execution_task_lease_heads \
         (execution_task_id, last_fencing_token, current_lease_id, updated_at) \
         VALUES ('tr_01ARZ3NDEKTSV4RRFFQ69G5FAW', 1, \
                 'ls_01ARZ3NDEKTSV4RRFFQ69G5FB0', 1)",
    )
    .execute(store.pool())
    .await
    .expect("valid lease head");

    let duplicate_active = sqlx::query(
        "INSERT INTO execution_task_leases \
         (id, execution_task_id, worker_incarnation_id, fencing_token, status, \
          acquired_at, expires_at, record_version) \
         VALUES ('ls_01ARZ3NDEKTSV4RRFFQ69G5FB1', 'tr_01ARZ3NDEKTSV4RRFFQ69G5FAW', \
                 'wi_01ARZ3NDEKTSV4RRFFQ69G5FAY', 2, 'active', 1, 3, 1)",
    )
    .execute(store.pool())
    .await;
    assert!(duplicate_active.is_err(), "one active lease per task");

    let mutate_identity = sqlx::query(
        "UPDATE execution_task_leases SET fencing_token = 2 \
         WHERE id = 'ls_01ARZ3NDEKTSV4RRFFQ69G5FB0'",
    )
    .execute(store.pool())
    .await;
    assert!(mutate_identity.is_err(), "lease identity is immutable");

    sqlx::query(
        "UPDATE execution_task_leases \
         SET status='released', terminal_at=2, terminal_reason='test', record_version=2 \
         WHERE id='ls_01ARZ3NDEKTSV4RRFFQ69G5FB0'",
    )
    .execute(store.pool())
    .await
    .expect("active to terminal");
    let reactivate = sqlx::query(
        "UPDATE execution_task_leases SET status='active', terminal_at=NULL, terminal_reason=NULL \
         WHERE id='ls_01ARZ3NDEKTSV4RRFFQ69G5FB0'",
    )
    .execute(store.pool())
    .await;
    assert!(reactivate.is_err(), "terminal lease cannot reactivate");

    let token_rollback = sqlx::query(
        "UPDATE execution_task_lease_heads SET last_fencing_token=0 \
         WHERE execution_task_id='tr_01ARZ3NDEKTSV4RRFFQ69G5FAW'",
    )
    .execute(store.pool())
    .await;
    assert!(token_rollback.is_err(), "head token cannot decrease");
    assert!(
        sqlx::query(
            "DELETE FROM execution_task_leases \
             WHERE id='ls_01ARZ3NDEKTSV4RRFFQ69G5FB0'",
        )
        .execute(store.pool())
        .await
        .is_err(),
        "lease history cannot be deleted"
    );
    assert!(
        sqlx::query(
            "DELETE FROM execution_task_lease_heads \
             WHERE execution_task_id='tr_01ARZ3NDEKTSV4RRFFQ69G5FAW'",
        )
        .execute(store.pool())
        .await
        .is_err(),
        "head history cannot be deleted"
    );
}
