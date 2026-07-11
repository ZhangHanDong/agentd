use std::path::Path;

use agentd_store::agent_chat_import::{
    self, AgentChatImportMode, AgentChatImportOptions, AgentChatMessageImportOptions,
    AgentChatTaskImportOptions,
};
use agentd_store::{SqliteStore, agent_repo};
use sqlx::Row;

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dir");
    }
    std::fs::write(path, content).expect("write file");
}

fn valid_checkout() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("agent-chat fixture");
    write_file(&dir.path().join("backend-v2.js"), "// fixture backend");
    write_file(&dir.path().join("lib/mcp-server-core.js"), "// fixture mcp");
    write_file(&dir.path().join("server.js"), "// fixture server");
    write_file(
        &dir.path().join("data/agents.json"),
        r#"{
  "codex-importer": {
    "name": "codex-importer",
    "agentId": "agent_codex_importer",
    "role": "implementer",
    "capability": "coding",
    "type": "codex",
    "agentModelVersion": "gpt-5",
    "tmux": "codex-importer:0.0",
    "homeDir": "/tmp/agent-chat/codex-importer",
    "workdir": "/tmp/agent-chat/codex-importer/workdir",
    "stateDir": "/tmp/agent-chat/codex-importer/state",
    "server": "local",
    "online": true
  }
}"#,
    );
    dir
}

fn valid_message_checkout() -> tempfile::TempDir {
    let dir = valid_checkout();
    write_file(
        &dir.path().join("data/groups.json"),
        r#"{
  "factory": {
    "name": "factory",
    "members": ["codex-worker", "codex-reviewer"],
    "createdAt": 900
  }
}
"#,
    );
    write_file(
        &dir.path().join("data/messages.json"),
        r#"[
  {
    "id": "msg_direct_read",
    "ts": 1000,
    "from": "alex",
    "to": "codex-worker",
    "group": null,
    "type": "human",
    "priority": "normal",
    "summary": "direct summary",
    "full": "direct full",
    "mentions": [],
    "reply_to": null,
    "source": "api"
  },
  {
    "id": "msg_group_read",
    "ts": 2000,
    "from": "codex-worker",
    "to": null,
    "group": "factory",
    "type": "inform",
    "priority": "normal",
    "summary": "group summary",
    "full": "group full",
    "mentions": ["codex-reviewer"],
    "reply_to": "msg_direct_read",
    "source": "api",
    "schema": {"kind": "task_update", "version": 1}
  }
]
"#,
    );
    write_file(
        &dir.path().join("data/cursors.json"),
        r#"{
  "codex-worker": {"inbox": 1000, "inboxId": "msg_direct_read", "groups": {}, "groupIds": {}},
  "codex-reviewer": {"inbox": 2000, "inboxId": "msg_group_read", "groups": {"factory": 2000}, "groupIds": {"factory": "msg_group_read"}}
}
"#,
    );
    dir
}

fn valid_task_checkout() -> tempfile::TempDir {
    let dir = valid_checkout();
    write_file(
        &dir.path().join("data/tasks.json"),
        r#"[
  {
    "id": "task_keep",
    "title": "Keep task",
    "description": "Imported task",
    "status": "created",
    "priority": "p1",
    "granularity": "task",
    "assignee": "codex-importer",
    "created_by": "alex",
    "created_at": "2026-07-09T09:00:00.000Z",
    "updated_at": "2026-07-09T09:00:00.000Z",
    "started_at": null,
    "completed_at": null,
    "heartbeat_at": null,
    "waiting_reason": null,
    "waiting_until": null,
    "parent_id": null,
    "labels": ["migration"],
    "health": null,
    "comments": []
  }
]
"#,
    );
    write_file(
        &dir.path().join("data/task_graphs.json"),
        r#"{
  "graph_keep": {
    "id": "graph_keep",
    "owner": "alex",
    "label": "Migration graph",
    "status": "active",
    "nodes": {
      "n1": {
        "id": "n1",
        "assignee": "codex-importer",
        "description": "Do the first node",
        "depends_on": [],
        "status": "pending"
      }
    },
    "createdAt": "2026-07-09T09:02:00.000Z",
    "updatedAt": "2026-07-09T09:02:00.000Z",
    "completedAt": null
  }
}
"#,
    );
    dir
}

async fn open_store() -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("target dir");
    let db = dir.path().join("agentd.db");
    let store = SqliteStore::connect(&db).await.expect("connect store");
    (store, dir)
}

async fn message_count(store: &SqliteStore, table: &str) -> i64 {
    let sql = format!("SELECT COUNT(*) AS count FROM {table}");
    sqlx::query_scalar::<_, i64>(&sql)
        .fetch_one(store.pool())
        .await
        .expect("count rows")
}

async fn compatibility_count(store: &SqliteStore, table: &str) -> i64 {
    let sql = format!("SELECT COUNT(*) AS count FROM {table}");
    sqlx::query_scalar::<_, i64>(&sql)
        .fetch_one(store.pool())
        .await
        .expect("count compatibility rows")
}

#[tokio::test]
async fn agent_chat_task_import_execute_preserves_task_and_graph_snapshots() {
    let source = valid_task_checkout();
    let (store, _target) = open_store().await;

    let report = agent_chat_import::import_tasks_from_agent_chat(
        store.pool(),
        source.path(),
        AgentChatTaskImportOptions {
            mode: AgentChatImportMode::Execute,
        },
    )
    .await
    .expect("task import succeeds");

    assert_eq!(report.tasks.imported, 1);
    assert_eq!(report.task_graphs.imported, 1);

    let task = sqlx::query(
        "SELECT id, status, assignee, raw_json FROM agent_chat_tasks WHERE id = 'task_keep'",
    )
    .fetch_one(store.pool())
    .await
    .expect("task_keep row");
    assert_eq!(task.get::<String, _>("id"), "task_keep");
    assert_eq!(task.get::<String, _>("status"), "created");
    assert_eq!(task.get::<String, _>("assignee"), "codex-importer");
    assert!(
        task.get::<String, _>("raw_json").contains("\"comments\""),
        "raw task JSON is preserved"
    );

    let graph =
        sqlx::query("SELECT id, raw_json FROM agent_chat_task_graphs WHERE id = 'graph_keep'")
            .fetch_one(store.pool())
            .await
            .expect("graph_keep row");
    assert_eq!(graph.get::<String, _>("id"), "graph_keep");
    assert!(
        graph.get::<String, _>("raw_json").contains("\"nodes\""),
        "raw task graph JSON is preserved"
    );
}

#[tokio::test]
async fn agent_chat_agent_import_rejects_malformed_agents_without_partial_writes() {
    let source = valid_checkout();
    write_file(&source.path().join("data/agents.json"), "{ not json");
    let (store, _target) = open_store().await;

    let err = agent_chat_import::import_agents_from_agent_chat(
        store.pool(),
        source.path(),
        AgentChatImportOptions {
            mode: AgentChatImportMode::Execute,
        },
    )
    .await
    .expect_err("malformed agents.json rejects import");
    assert!(
        err.to_string().contains("JSON") || err.to_string().contains("serde"),
        "unexpected error: {err}"
    );

    let agents = agent_repo::list_agents(store.pool())
        .await
        .expect("list agents");
    assert!(agents.is_empty(), "malformed import writes no agent rows");
}

#[tokio::test]
async fn agent_chat_message_import_rejects_malformed_messages_without_partial_writes() {
    let source = valid_message_checkout();
    write_file(&source.path().join("data/messages.json"), "{ not json");
    let (store, _target) = open_store().await;

    let err = agent_chat_import::import_messages_from_agent_chat(
        store.pool(),
        source.path(),
        AgentChatMessageImportOptions {
            mode: AgentChatImportMode::Execute,
        },
    )
    .await
    .expect_err("malformed messages.json rejects import");
    assert!(
        err.to_string().contains("JSON") || err.to_string().contains("serde"),
        "unexpected error: {err}"
    );

    assert_eq!(message_count(&store, "direct_messages").await, 0);
    assert_eq!(message_count(&store, "group_messages").await, 0);
}

#[tokio::test]
async fn agent_chat_task_import_rejects_malformed_tasks_without_partial_writes() {
    let source = valid_task_checkout();
    write_file(&source.path().join("data/tasks.json"), "{ not json");
    let (store, _target) = open_store().await;

    let err = agent_chat_import::import_tasks_from_agent_chat(
        store.pool(),
        source.path(),
        AgentChatTaskImportOptions {
            mode: AgentChatImportMode::Execute,
        },
    )
    .await
    .expect_err("malformed tasks.json rejects import");
    assert!(
        err.to_string().contains("JSON") || err.to_string().contains("serde"),
        "unexpected error: {err}"
    );

    assert_eq!(compatibility_count(&store, "agent_chat_tasks").await, 0);
    assert_eq!(
        compatibility_count(&store, "agent_chat_task_graphs").await,
        0
    );
}
