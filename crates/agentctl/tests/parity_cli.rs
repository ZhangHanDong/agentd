use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use agentd_store::{SqliteStore, message_repo};

const AGENT_CHAT_PATH: &str = "/Users/zhangalex/Work/Projects/consult/agent-chat";
const REQUIRED_CATEGORIES: &[&str] = &[
    "registry",
    "messaging",
    "task_graph",
    "scheduler",
    "runtime_launch",
    "dashboard_cli",
    "matrix_remote",
    "migration_cutover",
    "auth",
    "real_execution",
];
const ALLOWED_STATUS: &[&str] = &["covered", "partial", "missing", "deferred", "external"];

#[derive(Debug)]
struct ParityRow {
    capability: String,
    category: String,
    priority: String,
    status: String,
    source: String,
    decision: String,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn parity_map_path() -> PathBuf {
    repo_root().join("docs/parity/agent-chat-capability-map.md")
}

fn roadmap_path() -> PathBuf {
    repo_root().join("docs/plans/2026-07-08-agent-chat-replacement-roadmap.md")
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dir");
    }
    std::fs::write(path, content).expect("write file");
}

fn agent_chat_fixture(agent_names: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("agent-chat fixture");
    write_file(&dir.path().join("backend-v2.js"), "// fixture backend");
    write_file(&dir.path().join("lib/mcp-server-core.js"), "// fixture mcp");
    write_file(&dir.path().join("server.js"), "// fixture server");

    let agents = agent_names
        .iter()
        .map(|name| {
            format!(
                r#""{name}": {{
  "name": "{name}",
  "agentId": "agent_{name}",
  "role": "implementer",
  "capability": "coding",
  "type": "codex",
  "agentModelVersion": "gpt-5",
  "tmux": "{name}:0.0",
  "homeDir": "/tmp/agent-chat/{name}",
  "workdir": "/tmp/agent-chat/{name}/workdir",
  "stateDir": "/tmp/agent-chat/{name}/state",
  "server": "local",
  "online": true
}}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    write_file(
        &dir.path().join("data/agents.json"),
        &format!("{{\n{agents}\n}}\n"),
    );
    dir
}

fn agent_chat_messages_fixture(include_group: bool) -> tempfile::TempDir {
    let dir = agent_chat_fixture(&["codex-worker", "codex-reviewer"]);
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
    let group_row = if include_group {
        r#",
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
    "schema": {"kind": "task_update", "version": 1, "payload": {"ok": true}},
    "attachments": [{"path": "/tmp/group.txt", "name": "group.txt", "mime": "text/plain", "kind": "file", "size": 5, "staged": false}]
  }"#
    } else {
        ""
    };
    let direct_row = r#"{
    "id": "msg_direct_read",
    "ts": 1000,
    "from": "alex",
    "to": "codex-worker",
    "group": null,
    "type": "human",
    "priority": "high",
    "summary": "direct summary",
    "full": "direct full",
    "mentions": [],
    "reply_to": null,
    "source": "api",
    "sourceRoom": "operator-room",
    "senderMxid": null,
    "trustLevel": "operator",
    "fromId": "alex-id",
    "attachments": [{"path": "/tmp/direct.txt", "name": "direct.txt", "mime": "text/plain", "kind": "file", "size": 6, "staged": false}]
  }"#;
    write_file(
        &dir.path().join("data/messages.json"),
        &format!("[\n  {direct_row}{group_row}\n]\n"),
    );
    write_file(
        &dir.path().join("data/cursors.json"),
        r#"{
  "codex-worker": {
    "inbox": 1000,
    "inboxId": "msg_direct_read",
    "groups": {},
    "groupIds": {}
  },
  "codex-reviewer": {
    "inbox": 2000,
    "inboxId": "msg_group_read",
    "groups": {"factory": 2000},
    "groupIds": {"factory": "msg_group_read"}
  }
}
"#,
    );
    dir
}

fn agent_chat_tasks_fixture(include_second_task: bool, include_graph: bool) -> tempfile::TempDir {
    let dir = agent_chat_fixture(&["codex-worker", "codex-reviewer"]);
    let second_task = if include_second_task {
        r#",
  {
    "id": "task_missing",
    "title": "Second task",
    "description": "Second imported task",
    "status": "created",
    "priority": "p2",
    "granularity": "task",
    "assignee": "codex-reviewer",
    "created_by": "alex",
    "created_at": "2026-07-09T10:00:00.000Z",
    "updated_at": "2026-07-09T10:00:00.000Z",
    "started_at": null,
    "completed_at": null,
    "heartbeat_at": null,
    "waiting_reason": null,
    "waiting_until": null,
    "parent_id": null,
    "labels": ["review"],
    "health": null,
    "comments": []
  }"#
    } else {
        ""
    };
    write_file(
        &dir.path().join("data/tasks.json"),
        &format!(
            r#"[
  {{
    "id": "task_keep",
    "title": "Keep task",
    "description": "Imported task",
    "status": "created",
    "priority": "p1",
    "granularity": "task",
    "assignee": "codex-worker",
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
    "comments": [{{"author": "alex", "text": "carry me", "ts": "2026-07-09T09:01:00.000Z"}}]
  }}{second_task}
]
"#
        ),
    );
    if include_graph {
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
        "assignee": "codex-worker",
        "description": "Do the first node",
        "depends_on": [],
        "status": "pending",
        "result": null,
        "error": null
      }
    },
    "createdAt": "2026-07-09T09:02:00.000Z",
    "updatedAt": "2026-07-09T09:02:00.000Z",
    "completedAt": null
  }
}
"#,
        );
    }
    dir
}

fn agentctl(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agentctl"))
        .args(args)
        .output()
        .expect("spawn agentctl")
}

fn assert_imported_cursors_applied(db: &Path) {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    runtime.block_on(async {
        let store = SqliteStore::connect(db).await.expect("open imported db");
        let worker = message_repo::read_agent_inbox(
            store.pool(),
            "codex-worker",
            message_repo::InboxReadOptions { drain: false },
        )
        .await
        .expect("read worker inbox");
        assert!(
            worker.dm.is_empty() && worker.group.is_empty(),
            "direct inbox cursor import marks msg_direct_read as read"
        );

        let reviewer = message_repo::read_agent_inbox(
            store.pool(),
            "codex-reviewer",
            message_repo::InboxReadOptions { drain: false },
        )
        .await
        .expect("read reviewer inbox");
        assert!(
            reviewer.group.is_empty(),
            "inbox cursor import marks group mention msg_group_read as read"
        );

        let group = message_repo::read_group_messages(
            store.pool(),
            "factory",
            "codex-reviewer",
            message_repo::GroupReadOptions {
                limit: 10,
                unread_limit: Some(10),
                advance: message_repo::GroupReadAdvance::None,
            },
        )
        .await
        .expect("read group messages");
        assert_eq!(
            group.unread_total, 0,
            "group cursor import marks group history as read"
        );
        assert_eq!(group.read.len(), 1, "imported group row remains in history");
    });
}

fn parse_rows(markdown: &str) -> Vec<ParityRow> {
    markdown
        .lines()
        .filter(|line| line.trim_start().starts_with('|'))
        .filter(|line| !line.contains("---"))
        .skip(1)
        .map(|line| {
            let cells = line
                .trim()
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect::<Vec<_>>();
            assert_eq!(cells.len(), 7, "expected 7 columns in row: {line}");
            ParityRow {
                capability: cells[0].to_string(),
                category: cells[1].to_string(),
                priority: cells[2].to_string(),
                source: cells[3].to_string(),
                status: cells[4].to_string(),
                decision: cells[5].to_string(),
            }
        })
        .collect()
}

fn parity_rows() -> Vec<ParityRow> {
    let markdown = std::fs::read_to_string(parity_map_path()).expect("read parity map");
    parse_rows(&markdown)
}

fn count_entries(path: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                1 + count_entries(&path)
            } else {
                1
            }
        })
        .sum()
}

#[test]
fn parity_capability_map_has_required_rows_without_unknowns() {
    let rows = parity_rows();
    assert!(!rows.is_empty(), "parity map must contain capability rows");

    let mut required_by_category: BTreeMap<&str, usize> = REQUIRED_CATEGORIES
        .iter()
        .copied()
        .map(|category| (category, 0))
        .collect();
    let allowed = ALLOWED_STATUS.iter().copied().collect::<BTreeSet<_>>();

    for row in rows.iter().filter(|row| row.priority == "required") {
        assert!(
            allowed.contains(row.status.as_str()),
            "required row {} has unsupported status {}",
            row.capability,
            row.status
        );
        assert_ne!(row.status, "unknown", "unknown status is forbidden");
        assert!(
            row.source
                .starts_with("/Users/zhangalex/Work/Projects/consult/agent-chat/"),
            "row {} must cite an agent-chat source path: {}",
            row.capability,
            row.source
        );
        assert!(
            !row.decision.is_empty(),
            "row {} needs a replacement decision",
            row.capability
        );
        if let Some(count) = required_by_category.get_mut(row.category.as_str()) {
            *count += 1;
        }
    }

    let missing = required_by_category
        .into_iter()
        .filter_map(|(category, count)| (count == 0).then_some(category))
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "required categories missing from parity map: {missing:?}"
    );
}

#[test]
fn parity_capability_map_marks_real_codex_execution_partial_after_p201() {
    // Name kept for earlier spec selectors; p204-r9 advanced the row to covered.
    let rows = parity_rows();
    let row = rows
        .iter()
        .find(|row| row.capability == "real_codex_execution")
        .expect("real_codex_execution row");
    assert_eq!(row.status, "covered");
    assert!(
        row.decision.contains("p201") && row.decision.contains("role-prefixed"),
        "decision should mention p201 role-prefixed runtime selection progress: {}",
        row.decision
    );
}

#[test]
fn parity_capability_map_marks_real_codex_execution_partial_after_p202() {
    // Name kept for earlier spec selectors; p204-r9 advanced the row to covered.
    let rows = parity_rows();
    let row = rows
        .iter()
        .find(|row| row.capability == "real_codex_execution")
        .expect("real_codex_execution row");
    assert_eq!(row.status, "covered");
    assert!(
        row.decision.contains("p202")
            && row.decision.contains("Codex MCP")
            && row.decision.contains("launcher"),
        "decision should mention p202 Codex MCP launcher progress: {}",
        row.decision
    );
}

#[test]
fn parity_capability_map_marks_real_codex_execution_partial_after_p203() {
    // Name kept for earlier spec selectors; p204-r9 advanced the row to covered.
    let rows = parity_rows();
    let row = rows
        .iter()
        .find(|row| row.capability == "real_codex_execution")
        .expect("real_codex_execution row");
    assert_eq!(row.status, "covered");
    assert!(
        row.decision.contains("p203")
            && row.decision.contains("runtime matrix")
            && row.decision.contains("Codex"),
        "decision should mention p203 Codex runtime matrix progress: {}",
        row.decision
    );
}

#[test]
fn parity_capability_map_marks_real_codex_execution_covered_after_p204_r9() {
    let rows = parity_rows();
    let row = rows
        .iter()
        .find(|row| row.capability == "real_codex_execution")
        .expect("real_codex_execution row");

    assert_eq!(row.status, "covered");
    assert!(
        row.decision.contains("p204-codex-matrix-r9")
            && row.decision.contains("finished")
            && row
                .decision
                .contains(".agentd/real-execute-smoke/p204-codex-matrix-r9/summary.txt"),
        "decision should cite the finished real Codex execute evidence: {}",
        row.decision
    );
}

#[test]
fn parity_capability_map_records_p213_registry_lifecycle_progress() {
    let rows = parity_rows();
    let row = rows
        .iter()
        .find(|row| row.capability == "agent_registry_lifecycle")
        .expect("agent_registry_lifecycle row");

    assert_eq!(
        row.status, "partial",
        "p213 is progress but not full lifecycle parity"
    );
    for expected in [
        "p213",
        "register",
        "list",
        "inspect",
        "heartbeat",
        "offline",
    ] {
        assert!(
            row.decision.contains(expected),
            "decision should mention {expected}: {}",
            row.decision
        );
    }
    assert!(
        !row.decision.contains("start covered") && !row.decision.contains("runtime update covered"),
        "p213 must not claim start/runtime update coverage: {}",
        row.decision
    );
}

#[test]
fn parity_capability_map_records_p214_runtime_lifecycle_progress() {
    let rows = parity_rows();
    let registry = rows
        .iter()
        .find(|row| row.capability == "agent_registry_lifecycle")
        .expect("agent_registry_lifecycle row");
    let profiles = rows
        .iter()
        .find(|row| row.capability == "agent_runtime_profiles")
        .expect("agent_runtime_profiles row");

    assert_eq!(registry.status, "partial");
    for expected in ["p214", "launch-env", "start", "runtime observation"] {
        assert!(
            registry.decision.contains(expected),
            "registry decision should mention {expected}: {}",
            registry.decision
        );
    }

    assert_eq!(profiles.status, "partial");
    for expected in ["p214", "runtime profile", "launch-env"] {
        assert!(
            profiles.decision.contains(expected),
            "runtime profile decision should mention {expected}: {}",
            profiles.decision
        );
    }
}

#[test]
fn parity_capability_map_records_p215_auth_boundary_progress() {
    let rows = parity_rows();
    let row = rows
        .iter()
        .find(|row| row.capability == "api_auth_boundary")
        .expect("api_auth_boundary row");

    assert_eq!(row.status, "partial");
    for expected in [
        "p215",
        "bearer",
        "agent token",
        "local-only",
        "dashboard",
        "bridge",
        "relay",
        "import",
        "rotation",
    ] {
        assert!(
            row.decision.contains(expected),
            "auth decision should mention {expected}: {}",
            row.decision
        );
    }
}

#[test]
fn parity_agent_import_dry_run_reports_counts_without_creating_db() {
    let source = agent_chat_fixture(&["codex-importer", "codex-reviewer"]);
    let agents_path = source.path().join("data/agents.json");
    let before_agents = std::fs::read_to_string(&agents_path).expect("read agents before");
    let target = tempfile::tempdir().expect("target");
    let db = target.path().join("agentd.db");

    let out = agentctl(&[
        "parity",
        "import-agents",
        "--agent-chat",
        source.path().to_str().expect("source path"),
        "--db-path",
        db.to_str().expect("db path"),
    ]);

    assert_eq!(
        out.status.code(),
        Some(0),
        "dry-run exits 0; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for expected in ["mode=dry-run", "agents source=2 planned=2 imported=0"] {
        assert!(
            stdout.contains(expected),
            "stdout includes {expected}: {stdout}"
        );
    }
    assert!(!db.exists(), "dry-run must not create the target database");
    assert_eq!(
        before_agents,
        std::fs::read_to_string(&agents_path).expect("read agents after"),
        "dry-run must not mutate source agents.json"
    );
}

#[test]
fn parity_agent_import_execute_writes_supported_agents() {
    let source = agent_chat_fixture(&["codex-importer", "codex-reviewer"]);
    let target = tempfile::tempdir().expect("target");
    let db = target.path().join("agentd.db");

    let import = agentctl(&[
        "parity",
        "import-agents",
        "--agent-chat",
        source.path().to_str().expect("source path"),
        "--db-path",
        db.to_str().expect("db path"),
        "--execute",
    ]);
    assert_eq!(
        import.status.code(),
        Some(0),
        "execute exits 0; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&import.stdout),
        String::from_utf8_lossy(&import.stderr)
    );
    let stdout = String::from_utf8_lossy(&import.stdout);
    for expected in ["mode=execute", "agents source=2 planned=2 imported=2"] {
        assert!(
            stdout.contains(expected),
            "stdout includes {expected}: {stdout}"
        );
    }

    let audit = agentctl(&[
        "parity",
        "shadow-agents",
        "--agent-chat",
        source.path().to_str().expect("source path"),
        "--db-path",
        db.to_str().expect("db path"),
    ]);
    assert_eq!(
        audit.status.code(),
        Some(0),
        "shadow audit exits 0 after import; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&audit.stdout),
        String::from_utf8_lossy(&audit.stderr)
    );
    assert!(
        String::from_utf8_lossy(&audit.stdout).contains("drift: none"),
        "audit reports no drift"
    );
}

#[test]
fn parity_agent_shadow_audit_reports_missing_agents_without_mutating() {
    let partial = agent_chat_fixture(&["codex-importer"]);
    let full = agent_chat_fixture(&["codex-importer", "codex-reviewer"]);
    let target = tempfile::tempdir().expect("target");
    let db = target.path().join("agentd.db");

    let import = agentctl(&[
        "parity",
        "import-agents",
        "--agent-chat",
        partial.path().to_str().expect("partial source path"),
        "--db-path",
        db.to_str().expect("db path"),
        "--execute",
    ]);
    assert_eq!(import.status.code(), Some(0), "setup import succeeds");

    for round in 0..2 {
        let audit = agentctl(&[
            "parity",
            "shadow-agents",
            "--agent-chat",
            full.path().to_str().expect("full source path"),
            "--db-path",
            db.to_str().expect("db path"),
        ]);
        assert_eq!(
            audit.status.code(),
            Some(1),
            "audit round {round} reports drift; stdout: {}; stderr: {}",
            String::from_utf8_lossy(&audit.stdout),
            String::from_utf8_lossy(&audit.stderr)
        );
        let stdout = String::from_utf8_lossy(&audit.stdout);
        assert!(
            stdout.contains("missing agent: codex-reviewer"),
            "stdout names missing agent: {stdout}"
        );
    }
}

#[test]
fn parity_capability_map_records_p216_agent_import_shadow_progress() {
    let rows = parity_rows();
    let row = rows
        .iter()
        .find(|row| row.capability == "migration_shadow_cutover")
        .expect("migration_shadow_cutover row");

    assert_eq!(row.status, "partial");
    for expected in [
        "p216",
        "agents.json",
        "shadow audit",
        "messages",
        "tasks",
        "task graphs",
        "Matrix",
        "remote relay",
        "service cutover",
        "rollback",
        "token provisioning",
    ] {
        assert!(
            row.decision.contains(expected),
            "migration decision should mention {expected}: {}",
            row.decision
        );
    }
}

#[test]
fn parity_capability_map_records_p217_direct_inbox_progress() {
    let rows = parity_rows();
    let row = rows
        .iter()
        .find(|row| row.capability == "messaging_inbox")
        .expect("messaging_inbox row");

    assert_eq!(row.status, "partial");
    for expected in [
        "p217",
        "durable direct messages",
        "check_inbox",
        "preview",
        "drain",
        "group mentions",
        "send/post MCP",
        "attachments",
        "Matrix",
        "remote relay",
        "notification gates",
        "message import",
    ] {
        assert!(
            row.decision.contains(expected),
            "messaging_inbox decision should mention {expected}: {}",
            row.decision
        );
    }
}

#[test]
fn parity_capability_map_records_p218_send_message_progress() {
    let rows = parity_rows();
    let row = rows
        .iter()
        .find(|row| row.capability == "messaging_inbox")
        .expect("messaging_inbox row");

    assert_eq!(row.status, "partial");
    for expected in [
        "p218",
        "send_message",
        "explicit sender",
        "implicit identity",
        "group messaging",
        "post",
        "check_group",
        "attachments",
        "Matrix",
        "remote relay",
        "notification gates",
        "message import",
    ] {
        assert!(
            row.decision.contains(expected),
            "messaging_inbox decision should mention {expected}: {}",
            row.decision
        );
    }
}

#[test]
fn parity_capability_map_records_p219_stdio_identity_progress() {
    let rows = parity_rows();
    let row = rows
        .iter()
        .find(|row| row.capability == "messaging_inbox")
        .expect("messaging_inbox row");

    assert_eq!(row.status, "partial");
    for expected in [
        "p219",
        "stdio identity",
        "implicit direct sender",
        "implicit own inbox",
        "spoof",
        "group messaging",
        "attachments",
        "Matrix",
        "remote relay",
        "notification gates",
        "message import",
    ] {
        assert!(
            row.decision.contains(expected),
            "messaging_inbox decision should mention {expected}: {}",
            row.decision
        );
    }
}

#[test]
fn parity_capability_map_records_p220_group_messaging_progress() {
    let rows = parity_rows();
    let messaging = rows
        .iter()
        .find(|row| row.capability == "messaging_inbox")
        .expect("messaging_inbox row");
    let group = rows
        .iter()
        .find(|row| row.capability == "group_messaging")
        .expect("group_messaging row");

    assert_eq!(messaging.status, "partial");
    assert_eq!(group.status, "partial");
    for row in [messaging, group] {
        for expected in [
            "p220",
            "durable groups",
            "group mentions",
            "post",
            "check_group",
            "attachments",
            "Matrix",
            "remote relay",
            "notification gates",
            "import",
        ] {
            assert!(
                row.decision.contains(expected),
                "{} decision should mention {expected}: {}",
                row.capability,
                row.decision
            );
        }
    }
}

#[test]
fn parity_capability_map_records_p221_attachment_metadata_progress() {
    let rows = parity_rows();
    let row = rows
        .iter()
        .find(|row| row.capability == "attachments_media")
        .expect("attachments_media row");

    assert_eq!(row.status, "partial");
    for expected in [
        "p221",
        "local-file attachment",
        "20 MiB",
        "staged=false",
        "Matrix media transfer",
        "LocalPath localization",
        "dashboard previews",
    ] {
        assert!(
            row.decision.contains(expected),
            "attachments_media decision should mention {expected}: {}",
            row.decision
        );
    }
}

#[test]
fn parity_capability_map_records_p222_media_stage_fetch_progress() {
    let rows = parity_rows();
    let row = rows
        .iter()
        .find(|row| row.capability == "attachments_media")
        .expect("attachments_media row");

    assert_eq!(row.status, "partial");
    for expected in [
        "p222",
        "/api/media/stage",
        "/api/media/fetch",
        "Matrix media transfer",
        "remote relay media",
        "LocalPath localization",
        "dashboard previews",
    ] {
        assert!(
            row.decision.contains(expected),
            "attachments_media decision should mention {expected}: {}",
            row.decision
        );
    }
}

#[test]
fn parity_capability_map_records_p223_localpath_localization_progress() {
    let rows = parity_rows();
    let row = rows
        .iter()
        .find(|row| row.capability == "attachments_media")
        .expect("attachments_media row");

    assert_eq!(row.status, "partial");
    for expected in [
        "p223",
        "LocalPath localization",
        "stdio MCP proxy media cache",
        "Matrix media transfer",
        "remote relay media",
        "image sanitization",
        "dashboard previews",
        "import/cutover",
    ] {
        assert!(
            row.decision.contains(expected),
            "attachments_media decision should mention {expected}: {}",
            row.decision
        );
    }
}

#[test]
fn parity_message_import_dry_run_reports_counts_without_creating_db() {
    let source = agent_chat_messages_fixture(true);
    let paths = [
        source.path().join("data/messages.json"),
        source.path().join("data/groups.json"),
        source.path().join("data/cursors.json"),
    ];
    let before = paths
        .iter()
        .map(|path| std::fs::read_to_string(path).expect("read source before"))
        .collect::<Vec<_>>();
    let target = tempfile::tempdir().expect("target");
    let db = target.path().join("agentd.db");

    let out = agentctl(&[
        "parity",
        "import-messages",
        "--agent-chat",
        source.path().to_str().expect("source path"),
        "--db-path",
        db.to_str().expect("db path"),
    ]);

    assert_eq!(
        out.status.code(),
        Some(0),
        "dry-run exits 0; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for expected in [
        "agent-chat message import",
        "mode=dry-run",
        "messages source=2 planned=2 imported=0 direct=1 group=1",
        "groups source=1 planned=1 imported=0",
        "cursors source=2 planned=2 imported=0",
    ] {
        assert!(
            stdout.contains(expected),
            "stdout includes {expected}: {stdout}"
        );
    }
    assert!(!db.exists(), "dry-run must not create the target database");
    for (path, before) in paths.iter().zip(before) {
        assert_eq!(
            before,
            std::fs::read_to_string(path).expect("read source after"),
            "dry-run must not mutate {}",
            path.display()
        );
    }
}

#[test]
fn parity_message_import_execute_writes_messages_groups_and_cursors() {
    let source = agent_chat_messages_fixture(true);
    let target = tempfile::tempdir().expect("target");
    let db = target.path().join("agentd.db");

    let import = agentctl(&[
        "parity",
        "import-messages",
        "--agent-chat",
        source.path().to_str().expect("source path"),
        "--db-path",
        db.to_str().expect("db path"),
        "--execute",
    ]);
    assert_eq!(
        import.status.code(),
        Some(0),
        "execute exits 0; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&import.stdout),
        String::from_utf8_lossy(&import.stderr)
    );
    let stdout = String::from_utf8_lossy(&import.stdout);
    for expected in [
        "mode=execute",
        "messages source=2 planned=2 imported=2 direct=1 group=1",
        "groups source=1 planned=1 imported=1",
        "cursors source=2 planned=2 imported=2",
    ] {
        assert!(
            stdout.contains(expected),
            "stdout includes {expected}: {stdout}"
        );
    }

    let audit = agentctl(&[
        "parity",
        "shadow-messages",
        "--agent-chat",
        source.path().to_str().expect("source path"),
        "--db-path",
        db.to_str().expect("db path"),
    ]);
    assert_eq!(
        audit.status.code(),
        Some(0),
        "shadow audit exits 0 after import; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&audit.stdout),
        String::from_utf8_lossy(&audit.stderr)
    );
    assert!(
        String::from_utf8_lossy(&audit.stdout).contains("drift: none"),
        "audit reports no drift"
    );
    assert_imported_cursors_applied(&db);
}

#[test]
fn parity_message_shadow_audit_reports_missing_messages_without_mutating() {
    let partial = agent_chat_messages_fixture(false);
    let full = agent_chat_messages_fixture(true);
    let target = tempfile::tempdir().expect("target");
    let db = target.path().join("agentd.db");

    let import = agentctl(&[
        "parity",
        "import-messages",
        "--agent-chat",
        partial.path().to_str().expect("partial source path"),
        "--db-path",
        db.to_str().expect("db path"),
        "--execute",
    ]);
    assert_eq!(
        import.status.code(),
        Some(0),
        "setup import succeeds; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&import.stdout),
        String::from_utf8_lossy(&import.stderr)
    );

    for round in 0..2 {
        let audit = agentctl(&[
            "parity",
            "shadow-messages",
            "--agent-chat",
            full.path().to_str().expect("full source path"),
            "--db-path",
            db.to_str().expect("db path"),
        ]);
        assert_eq!(
            audit.status.code(),
            Some(1),
            "audit round {round} reports drift; stdout: {}; stderr: {}",
            String::from_utf8_lossy(&audit.stdout),
            String::from_utf8_lossy(&audit.stderr)
        );
        let stdout = String::from_utf8_lossy(&audit.stdout);
        assert!(
            stdout.contains("missing group message: msg_group_read"),
            "stdout names missing message: {stdout}"
        );
    }
}

#[test]
fn parity_task_import_dry_run_reports_counts_without_creating_db() {
    let source = agent_chat_tasks_fixture(true, true);
    let paths = [
        source.path().join("data/tasks.json"),
        source.path().join("data/task_graphs.json"),
    ];
    let before = paths
        .iter()
        .map(|path| std::fs::read_to_string(path).expect("read source before"))
        .collect::<Vec<_>>();
    let target = tempfile::tempdir().expect("target");
    let db = target.path().join("agentd.db");

    let out = agentctl(&[
        "parity",
        "import-tasks",
        "--agent-chat",
        source.path().to_str().expect("source path"),
        "--db-path",
        db.to_str().expect("db path"),
    ]);

    assert_eq!(
        out.status.code(),
        Some(0),
        "dry-run exits 0; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for expected in [
        "agent-chat task import",
        "mode=dry-run",
        "tasks source=2 planned=2 imported=0",
        "task_graphs source=1 planned=1 imported=0",
    ] {
        assert!(
            stdout.contains(expected),
            "stdout includes {expected}: {stdout}"
        );
    }
    assert!(!db.exists(), "dry-run must not create the target database");
    for (path, before) in paths.iter().zip(before) {
        assert_eq!(
            before,
            std::fs::read_to_string(path).expect("read source after"),
            "dry-run must not mutate {}",
            path.display()
        );
    }
}

#[test]
fn parity_task_import_execute_writes_tasks_and_graphs() {
    let source = agent_chat_tasks_fixture(true, true);
    let target = tempfile::tempdir().expect("target");
    let db = target.path().join("agentd.db");

    let import = agentctl(&[
        "parity",
        "import-tasks",
        "--agent-chat",
        source.path().to_str().expect("source path"),
        "--db-path",
        db.to_str().expect("db path"),
        "--execute",
    ]);
    assert_eq!(
        import.status.code(),
        Some(0),
        "execute exits 0; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&import.stdout),
        String::from_utf8_lossy(&import.stderr)
    );
    let stdout = String::from_utf8_lossy(&import.stdout);
    for expected in [
        "mode=execute",
        "tasks source=2 planned=2 imported=2",
        "task_graphs source=1 planned=1 imported=1",
    ] {
        assert!(
            stdout.contains(expected),
            "stdout includes {expected}: {stdout}"
        );
    }

    let audit = agentctl(&[
        "parity",
        "shadow-tasks",
        "--agent-chat",
        source.path().to_str().expect("source path"),
        "--db-path",
        db.to_str().expect("db path"),
    ]);
    assert_eq!(
        audit.status.code(),
        Some(0),
        "shadow audit exits 0 after import; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&audit.stdout),
        String::from_utf8_lossy(&audit.stderr)
    );
    assert!(
        String::from_utf8_lossy(&audit.stdout).contains("drift: none"),
        "audit reports no drift"
    );
}

#[test]
fn parity_task_shadow_audit_reports_missing_tasks_without_mutating() {
    let partial = agent_chat_tasks_fixture(false, false);
    let full = agent_chat_tasks_fixture(true, true);
    let target = tempfile::tempdir().expect("target");
    let db = target.path().join("agentd.db");

    let import = agentctl(&[
        "parity",
        "import-tasks",
        "--agent-chat",
        partial.path().to_str().expect("partial source path"),
        "--db-path",
        db.to_str().expect("db path"),
        "--execute",
    ]);
    assert_eq!(
        import.status.code(),
        Some(0),
        "setup import succeeds; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&import.stdout),
        String::from_utf8_lossy(&import.stderr)
    );

    for round in 0..2 {
        let audit = agentctl(&[
            "parity",
            "shadow-tasks",
            "--agent-chat",
            full.path().to_str().expect("full source path"),
            "--db-path",
            db.to_str().expect("db path"),
        ]);
        assert_eq!(
            audit.status.code(),
            Some(1),
            "audit round {round} reports drift; stdout: {}; stderr: {}",
            String::from_utf8_lossy(&audit.stdout),
            String::from_utf8_lossy(&audit.stderr)
        );
        let stdout = String::from_utf8_lossy(&audit.stdout);
        for expected in [
            "missing task: task_missing",
            "missing task graph: graph_keep",
        ] {
            assert!(
                stdout.contains(expected),
                "stdout names {expected}: {stdout}"
            );
        }
    }
}

#[test]
fn parity_capability_map_records_p224_message_import_shadow_progress() {
    let rows = parity_rows();
    let messaging = rows
        .iter()
        .find(|row| row.capability == "messaging_inbox")
        .expect("messaging_inbox row");
    let group = rows
        .iter()
        .find(|row| row.capability == "group_messaging")
        .expect("group_messaging row");
    let migration = rows
        .iter()
        .find(|row| row.capability == "migration_shadow_cutover")
        .expect("migration_shadow_cutover row");

    for row in [messaging, group, migration] {
        assert_eq!(row.status, "partial");
        for expected in [
            "p224",
            "message import",
            "shadow",
            "cursor",
            "Matrix",
            "remote relay",
            "notification gates",
            "dashboard message",
            "task",
            "service cutover",
            "rollback",
            "token provisioning",
        ] {
            assert!(
                row.decision.contains(expected),
                "{} decision should mention {expected}: {}",
                row.capability,
                row.decision
            );
        }
    }
}

#[test]
fn parity_capability_map_records_p225_task_import_shadow_progress() {
    let rows = parity_rows();
    let task_graph = rows
        .iter()
        .find(|row| row.capability == "task_graph_coordination")
        .expect("task_graph_coordination row");
    let migration = rows
        .iter()
        .find(|row| row.capability == "migration_shadow_cutover")
        .expect("migration_shadow_cutover row");

    for row in [task_graph, migration] {
        assert_eq!(row.status, "partial");
        for expected in [
            "p225",
            "task import",
            "task-graph snapshot",
            "shadow",
            "live task CRUD",
            "DAG dispatch",
            "scheduler",
            "dashboard",
            "Matrix",
            "remote relay",
            "service cutover",
            "rollback",
            "token provisioning",
        ] {
            assert!(
                row.decision.contains(expected),
                "{} decision should mention {expected}: {}",
                row.capability,
                row.decision
            );
        }
    }
}

#[test]
fn parity_capability_map_records_p226_live_task_crud_progress() {
    let rows = parity_rows();
    let task_graph = rows
        .iter()
        .find(|row| row.capability == "task_graph_coordination")
        .expect("task_graph_coordination row");
    let migration = rows
        .iter()
        .find(|row| row.capability == "migration_shadow_cutover")
        .expect("migration_shadow_cutover row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");

    for row in [task_graph, migration] {
        assert_eq!(row.status, "partial");
        for expected in [
            "p226",
            "live",
            "/api/tasks",
            "CRUD",
            "DAG dispatch",
            "scheduler",
            "dashboard",
            "Matrix",
            "remote relay",
            "service cutover",
            "rollback",
            "token provisioning",
        ] {
            assert!(
                row.decision.contains(expected),
                "{} decision should mention {expected}: {}",
                row.capability,
                row.decision
            );
        }
    }

    for expected in [
        "p226",
        "live `/api/tasks` CRUD",
        "task-graph DAG",
        "scheduler",
        "dashboard",
        "Matrix/remote relay",
        "service cutover",
        "rollback",
        "token provisioning",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }
}

#[test]
fn parity_capability_map_records_p227_live_task_graph_progress() {
    let rows = parity_rows();
    let task_graph = rows
        .iter()
        .find(|row| row.capability == "task_graph_coordination")
        .expect("task_graph_coordination row");
    let migration = rows
        .iter()
        .find(|row| row.capability == "migration_shadow_cutover")
        .expect("migration_shadow_cutover row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");

    for row in [task_graph, migration] {
        assert_eq!(row.status, "partial");
        for expected in [
            "p227",
            "live",
            "/api/task-graphs",
            "dispatch",
            "scheduler",
            "dashboard",
            "Matrix",
            "remote relay",
            "service cutover",
            "rollback",
            "token provisioning",
        ] {
            assert!(
                row.decision.contains(expected),
                "{} decision should mention {expected}: {}",
                row.capability,
                row.decision
            );
        }
    }

    for expected in [
        "p227",
        "live `/api/task-graphs`",
        "task-graph dispatch",
        "scheduler",
        "dashboard",
        "Matrix/remote relay",
        "service cutover",
        "rollback",
        "token provisioning",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }
}

#[test]
fn parity_capability_map_records_p228_pool_scheduler_progress() {
    let rows = parity_rows();
    let scheduler = rows
        .iter()
        .find(|row| row.capability == "pool_scheduler")
        .expect("pool_scheduler row");
    let task_graph = rows
        .iter()
        .find(|row| row.capability == "task_graph_coordination")
        .expect("task_graph_coordination row");
    let migration = rows
        .iter()
        .find(|row| row.capability == "migration_shadow_cutover")
        .expect("migration_shadow_cutover row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");

    assert_eq!(scheduler.status, "partial");
    for expected in [
        "p228",
        "/api/pool",
        "/api/dispatch",
        "reservations",
        "release",
        "queue",
        "provision",
        "task-graph",
        "workflow",
        "dashboard",
        "Matrix",
        "remote relay",
        "cutover",
        "rollback",
        "token provisioning",
    ] {
        assert!(
            scheduler.decision.contains(expected),
            "pool_scheduler decision should mention {expected}: {}",
            scheduler.decision
        );
    }

    for row in [task_graph, migration] {
        assert_eq!(row.status, "partial");
        for expected in ["p228", "scheduler", "dashboard", "Matrix", "remote relay"] {
            assert!(
                row.decision.contains(expected),
                "{} decision should mention {expected}: {}",
                row.capability,
                row.decision
            );
        }
    }

    for expected in [
        "p228",
        "pool scheduler",
        "`/api/pool`",
        "`/api/dispatch`",
        "durable reservations",
        "release",
        "queue",
        "provision",
        "task-graph/workflow integration",
        "dashboard",
        "Matrix/remote relay",
        "cutover",
        "rollback",
        "token provisioning",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }
}

#[test]
fn parity_capability_map_records_p229_task_graph_scheduler_progress() {
    let rows = parity_rows();
    let scheduler = rows
        .iter()
        .find(|row| row.capability == "pool_scheduler")
        .expect("pool_scheduler row");
    let task_graph = rows
        .iter()
        .find(|row| row.capability == "task_graph_coordination")
        .expect("task_graph_coordination row");
    let migration = rows
        .iter()
        .find(|row| row.capability == "migration_shadow_cutover")
        .expect("migration_shadow_cutover row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");

    for row in [scheduler, task_graph, migration] {
        assert_eq!(row.status, "partial");
        for expected in [
            "p229",
            "task-graph scheduler integration",
            "scheduled nodes",
            "reservation metadata",
            "queued ticket drain",
            "result-time release",
            "workflow scheduler allocation",
            "dashboard",
            "Matrix",
            "remote relay",
            "cutover",
            "rollback",
            "token provisioning",
        ] {
            assert!(
                row.decision.contains(expected),
                "{} decision should mention {expected}: {}",
                row.capability,
                row.decision
            );
        }
    }

    for expected in [
        "p229",
        "task-graph scheduler integration",
        "scheduled nodes",
        "reservation metadata",
        "queued ticket drain",
        "result-time release",
        "workflow scheduler allocation",
        "dashboard",
        "Matrix/remote relay",
        "cutover",
        "rollback",
        "token provisioning",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }
}

#[test]
fn parity_capability_map_records_p230_workflow_scheduler_progress() {
    let rows = parity_rows();
    let scheduler = rows
        .iter()
        .find(|row| row.capability == "pool_scheduler")
        .expect("pool_scheduler row");
    let task_graph = rows
        .iter()
        .find(|row| row.capability == "task_graph_coordination")
        .expect("task_graph_coordination row");
    let migration = rows
        .iter()
        .find(|row| row.capability == "migration_shadow_cutover")
        .expect("migration_shadow_cutover row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");

    for row in [scheduler, task_graph, migration] {
        assert_eq!(row.status, "partial");
        for expected in [
            "p230",
            "workflow scheduler allocation",
            "codergen",
            "fan_out",
            "scheduler metadata in run events",
            "release behavior",
            "dashboard",
            "Matrix",
            "remote relay",
            "existing-pane prompt reuse",
            "queued workflow wakeups",
            "cutover",
            "rollback",
            "token provisioning",
        ] {
            assert!(
                row.decision.contains(expected),
                "{} decision should mention {expected}: {}",
                row.capability,
                row.decision
            );
        }
    }

    for expected in [
        "p230",
        "workflow scheduler allocation",
        "codergen",
        "fan_out",
        "run_parked",
        "release reservations",
        "existing-pane prompt reuse",
        "queued workflow wakeups",
        "Matrix/remote relay",
        "cutover",
        "rollback",
        "token provisioning",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }
}

#[test]
fn parity_capability_map_records_p231_existing_pane_reuse_progress() {
    let rows = parity_rows();
    let scheduler = rows
        .iter()
        .find(|row| row.capability == "pool_scheduler")
        .expect("pool_scheduler row");
    let task_graph = rows
        .iter()
        .find(|row| row.capability == "task_graph_coordination")
        .expect("task_graph_coordination row");
    let runtime_launch = rows
        .iter()
        .find(|row| row.capability == "runtime_launch_tmux")
        .expect("runtime_launch_tmux row");
    let migration = rows
        .iter()
        .find(|row| row.capability == "migration_shadow_cutover")
        .expect("migration_shadow_cutover row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");

    for row in [scheduler, task_graph, runtime_launch, migration] {
        assert_eq!(row.status, "partial");
        for expected in [
            "p231",
            "existing-pane prompt reuse",
            "tmux rebind",
            "routed online agents",
            "no duplicate spawn",
            "queued workflow wakeups",
            "dashboard",
            "Matrix",
            "remote relay",
            "cutover",
            "rollback",
            "token provisioning",
        ] {
            assert!(
                row.decision.contains(expected),
                "{} decision should mention {expected}: {}",
                row.capability,
                row.decision
            );
        }
    }

    for expected in [
        "p231",
        "existing-pane prompt reuse",
        "tmux rebind",
        "routed online agents",
        "no duplicate spawn",
        "queued workflow wakeups",
        "Matrix/remote relay",
        "cutover",
        "rollback",
        "token provisioning",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }
}

#[test]
fn parity_capability_map_records_p232_queued_codergen_wakeup_progress() {
    let rows = parity_rows();
    let scheduler = rows
        .iter()
        .find(|row| row.capability == "pool_scheduler")
        .expect("pool_scheduler row");
    let task_graph = rows
        .iter()
        .find(|row| row.capability == "task_graph_coordination")
        .expect("task_graph_coordination row");
    let runtime_launch = rows
        .iter()
        .find(|row| row.capability == "runtime_launch_tmux")
        .expect("runtime_launch_tmux row");
    let migration = rows
        .iter()
        .find(|row| row.capability == "migration_shadow_cutover")
        .expect("migration_shadow_cutover row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");

    for row in [scheduler, task_graph, runtime_launch, migration] {
        assert_eq!(row.status, "partial");
        for expected in [
            "p232",
            "queued codergen workflow wakeup",
            "release-drain dispatch",
            "duplicate-dispatch suppression",
            "fan_out queued wakeups",
            "dashboard",
            "Matrix",
            "remote relay",
            "cutover",
            "rollback",
            "notification gates",
            "token provisioning",
        ] {
            assert!(
                row.decision.contains(expected),
                "{} decision should mention {expected}: {}",
                row.capability,
                row.decision
            );
        }
    }

    for expected in [
        "p232",
        "queued codergen workflow wakeup",
        "release-drain dispatch",
        "duplicate-dispatch suppression",
        "fan_out queued wakeups",
        "Matrix/remote relay",
        "cutover",
        "rollback",
        "notification gates",
        "token provisioning",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }
}

#[test]
fn parity_capability_map_records_p233_queued_fanout_wakeup_progress() {
    let rows = parity_rows();
    let scheduler = rows
        .iter()
        .find(|row| row.capability == "pool_scheduler")
        .expect("pool_scheduler row");
    let task_graph = rows
        .iter()
        .find(|row| row.capability == "task_graph_coordination")
        .expect("task_graph_coordination row");
    let runtime_launch = rows
        .iter()
        .find(|row| row.capability == "runtime_launch_tmux")
        .expect("runtime_launch_tmux row");
    let migration = rows
        .iter()
        .find(|row| row.capability == "migration_shadow_cutover")
        .expect("migration_shadow_cutover row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");

    for row in [scheduler, task_graph, runtime_launch, migration] {
        assert_eq!(row.status, "partial");
        for expected in [
            "p233",
            "queued fan_out reviewer wakeup",
            "release-drain dispatch",
            "duplicate-dispatch suppression",
            "dashboard",
            "Matrix",
            "remote relay",
            "cutover",
            "rollback",
            "notification gates",
            "token provisioning",
        ] {
            assert!(
                row.decision.contains(expected),
                "{} decision should mention {expected}: {}",
                row.capability,
                row.decision
            );
        }
    }

    for expected in [
        "p233",
        "queued fan_out reviewer wakeup",
        "release-drain dispatch",
        "duplicate-dispatch suppression",
        "Matrix/remote relay",
        "cutover",
        "rollback",
        "notification gates",
        "token provisioning",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }
}

#[test]
fn parity_capability_map_records_p234_agent_lifecycle_progress() {
    let rows = parity_rows();
    let registry = rows
        .iter()
        .find(|row| row.capability == "agent_registry_lifecycle")
        .expect("agent_registry_lifecycle row");
    let runtime_launch = rows
        .iter()
        .find(|row| row.capability == "runtime_launch_tmux")
        .expect("runtime_launch_tmux row");
    let migration = rows
        .iter()
        .find(|row| row.capability == "migration_shadow_cutover")
        .expect("migration_shadow_cutover row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");

    for row in [registry, runtime_launch, migration] {
        assert_eq!(row.status, "partial");
        for expected in [
            "p234",
            "down",
            "rebind",
            "session recovery",
            "runtime lifecycle metadata",
            "dashboard",
            "Matrix",
            "remote relay",
            "cutover",
            "rollback",
            "token provisioning",
            "agent home",
        ] {
            assert!(
                row.decision.contains(expected),
                "{} decision should mention {expected}: {}",
                row.capability,
                row.decision
            );
        }
    }

    for expected in [
        "p234",
        "down",
        "rebind",
        "session recovery",
        "runtime lifecycle metadata",
        "Matrix/remote relay",
        "cutover",
        "rollback",
        "token provisioning",
        "agent home",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }
}

#[test]
fn parity_capability_map_records_p235_remote_relay_progress() {
    let rows = parity_rows();
    let remote = rows
        .iter()
        .find(|row| row.capability == "remote_relay")
        .expect("remote_relay row");
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");

    assert_eq!(remote.status, "partial");
    for expected in [
        "p235",
        "server heartbeat",
        "delivery-event audit",
        "message wakeup stream",
        "remote package",
        "tmux injection",
        "Matrix",
    ] {
        assert!(
            remote.decision.contains(expected),
            "remote relay decision should mention {expected}: {}",
            remote.decision
        );
    }

    assert!(
        matches!(matrix.status.as_str(), "missing" | "partial"),
        "matrix bridge row should remain explicit: {}",
        matrix.decision
    );
    assert!(
        matrix.decision.contains("matrix-sdk-adapter"),
        "matrix bridge row should remain explicit: {}",
        matrix.decision
    );

    for expected in [
        "p235",
        "server heartbeat",
        "delivery-event audit",
        "message wakeup stream",
        "remote package",
        "tmux injection",
        "Matrix bridge remained missing",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }
}

#[test]
fn parity_capability_map_records_p236_matrix_bridge_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let remote = rows
        .iter()
        .find(|row| row.capability == "remote_relay")
        .expect("remote_relay row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p236",
        "room trust",
        "Matrix inbound",
        "Matrix outbox",
        "external bridge contract",
        "puppet",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    assert_eq!(remote.status, "partial");
    assert!(
        remote.decision.contains("remote package"),
        "remote relay row should remain partial and name remaining package gap: {}",
        remote.decision
    );

    for expected in [
        "p236",
        "room trust",
        "Matrix inbound",
        "Matrix outbox",
        "real Matrix bridge process",
        "Matrix bridge is partial",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }
}

#[test]
fn parity_capability_map_records_p237_matrix_bridge_scaffold_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p237",
        "agentd-matrix",
        "bridge runtime scaffold",
        "cursor",
        "matrix-sdk-adapter",
        "SdkMatrixClient",
        "puppet",
        "full timeline parsing",
        "Matrix media",
        "cutover",
        "rollback",
        "token provisioning",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p237",
        "agentd-matrix",
        "bridge runtime scaffold",
        "fake backend",
        "fake Matrix transport",
        "Matrix bridge remains partial",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }
}

#[test]
fn parity_capability_map_records_p238_matrix_http_backend_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p238",
        "AgentdHttpBackend",
        "JSON cursor state",
        "standard-library HTTP",
        "real Matrix SDK",
        "puppet",
        "Matrix media",
        "cutover",
        "rollback",
        "token provisioning",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p238",
        "AgentdHttpBackend",
        "JSON cursor state",
        "standard-library HTTP",
        "Matrix bridge remains partial",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }
}

#[test]
fn parity_capability_map_records_p239_matrix_bridge_once_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p239",
        "matrix-bridge-once",
        "file-backed Matrix transport",
        "target-to-room",
        "real Matrix SDK",
        "puppet",
        "Matrix media",
        "cutover",
        "rollback",
        "token provisioning",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p239",
        "matrix-bridge-once",
        "file-backed Matrix transport",
        "target-to-room",
        "Matrix bridge remains partial",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }
}

#[test]
fn parity_capability_map_records_p240_matrix_client_adapter_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p240",
        "MatrixClientPort",
        "MatrixClientBridgeTransport",
        "trust-mode invite",
        "loop suppression",
        "real Matrix SDK",
        "puppet",
        "Matrix media",
        "cutover",
        "rollback",
        "token provisioning",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p240",
        "MatrixClientPort",
        "MatrixClientBridgeTransport",
        "trust-mode invite",
        "loop suppression",
        "Matrix bridge remains partial",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }
}

#[test]
fn parity_capability_map_records_p241_matrix_sdk_adapter_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p241",
        "matrix-sdk-adapter",
        "SdkMatrixClient",
        "puppet",
        "full timeline parsing",
        "Matrix media",
        "cutover",
        "rollback",
        "token provisioning",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p241",
        "matrix-sdk-adapter",
        "SdkMatrixClient",
        "feature-gated",
        "Matrix bridge remains partial",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }
}

#[test]
fn parity_capability_map_records_p242_matrix_timeline_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p242",
        "SDK timeline text parsing",
        "MatrixClientTextMessage",
        "puppet",
        "Matrix media",
        "cutover",
        "rollback",
        "token provisioning",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p242",
        "SDK timeline text parsing",
        "m.room.message",
        "Matrix bridge remains partial",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }
}

#[test]
fn parity_capability_map_records_p243_matrix_puppet_identity_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let matrix_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-matrix/src/lib.rs"))
            .expect("read agentd-matrix source");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p243",
        "puppet identity mapping",
        "MatrixPuppetDirectory",
        "MatrixPuppetAccount",
        "account registration",
        "Matrix media",
        "cutover",
        "rollback",
        "token provisioning",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p243",
        "puppet identity mapping",
        "Matrix bridge remains partial",
        "account registration",
        "token provisioning",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in [
        "matrix_server_name",
        "known_agent_names",
        "skip_agent_names",
        "prefix-only fallback",
    ] {
        assert!(
            matrix_source.contains(expected),
            "agentd-matrix source should mention {expected}"
        );
    }
}

#[test]
fn parity_capability_map_records_p244_matrix_puppet_provisioning_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let matrix_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-matrix/src/lib.rs"))
            .expect("read agentd-matrix source");
    let matrix_manifest =
        std::fs::read_to_string(repo_root().join("crates/agentd-matrix/Cargo.toml"))
            .expect("read agentd-matrix manifest");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p244",
        "puppet account provisioning plan",
        "MatrixPuppetProvisioningPlan",
        "MatrixPuppetTokenState",
        "real account registration",
        "Matrix media",
        "cutover",
        "rollback",
        "token rotation",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p244",
        "puppet account provisioning plan",
        "password candidate",
        "Matrix bridge remains partial",
        "real account registration",
        "token rotation",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in [
        "MatrixPuppetProvisioningPlan",
        "MatrixPuppetTokenState",
        "MatrixPuppetProvisioningConfig",
        "MatrixPuppetRegistrationAuth",
    ] {
        assert!(
            matrix_source.contains(expected),
            "agentd-matrix source should mention {expected}"
        );
    }

    assert!(
        matrix_manifest.contains("sha2"),
        "agentd-matrix manifest should include sha2 for agent-chat-compatible password derivation"
    );
}

#[test]
fn parity_capability_map_records_p245_matrix_puppet_account_executor_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let matrix_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-matrix/src/lib.rs"))
            .expect("read agentd-matrix source");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p245",
        "puppet account executor boundary",
        "MatrixPuppetAccountExecutor",
        "MatrixPuppetAccountPort",
        "real Matrix HTTP account registration",
        "Matrix media",
        "cutover",
        "rollback",
        "token rotation",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p245",
        "puppet account executor boundary",
        "MatrixPuppetAccountExecutor",
        "Matrix bridge remains partial",
        "real Matrix HTTP account registration",
        "token rotation",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in [
        "MatrixPuppetAccountExecutor",
        "MatrixPuppetAccountPort",
        "MatrixPuppetTokenSink",
    ] {
        assert!(
            matrix_source.contains(expected),
            "agentd-matrix source should mention {expected}"
        );
    }
}

#[test]
fn parity_capability_map_records_p246_matrix_puppet_http_account_port_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let matrix_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-matrix/src/lib.rs"))
            .expect("read agentd-matrix source");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p246",
        "MatrixPuppetHttpAccountPort",
        "MatrixPuppetHttpAccountConfig",
        "Matrix media",
        "cutover",
        "rollback",
        "token rotation",
        "service packaging",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p246",
        "MatrixPuppetHttpAccountPort",
        "Matrix bridge remains partial",
        "service packaging",
        "token rotation",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in [
        "MatrixPuppetHttpAccountConfig",
        "MatrixPuppetHttpAccountPort",
        "_matrix/client/v3/register",
        "_matrix/client/v3/login",
        "_matrix/client/v3/account/whoami",
    ] {
        assert!(
            matrix_source.contains(expected),
            "agentd-matrix source should mention {expected}"
        );
    }
}

#[test]
fn parity_capability_map_records_p247_matrix_puppet_http_account_provisioner_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let matrix_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-matrix/src/lib.rs"))
            .expect("read agentd-matrix source");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p247",
        "MatrixPuppetHttpAccountProvisioner",
        "durable token-store backends",
        "Matrix media",
        "cutover",
        "rollback",
        "token rotation",
        "service packaging",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p247",
        "MatrixPuppetHttpAccountProvisioner",
        "Matrix bridge remains partial",
        "durable token-store backends",
        "service packaging",
        "token rotation",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in [
        "MatrixPuppetHttpAccountProvisioner",
        "MatrixPuppetHttpAccountPort",
        "MatrixPuppetAccountExecutor",
    ] {
        assert!(
            matrix_source.contains(expected),
            "agentd-matrix source should mention {expected}"
        );
    }
}

#[test]
fn parity_capability_map_records_p248_matrix_puppet_token_file_store_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let matrix_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-matrix/src/lib.rs"))
            .expect("read agentd-matrix source");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p248",
        "MatrixPuppetTokenFileStore",
        "agentTokens",
        "daemon/SDK account provisioning assembly",
        "Matrix media",
        "cutover",
        "rollback",
        "token rotation",
        "service packaging",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p248",
        "MatrixPuppetTokenFileStore",
        "agentTokens",
        "Matrix bridge remains partial",
        "daemon/SDK account provisioning assembly",
        "service packaging",
        "token rotation",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in [
        "MatrixPuppetTokenFileStore",
        "MatrixPuppetTokenSink",
        "agentTokens",
    ] {
        assert!(
            matrix_source.contains(expected),
            "agentd-matrix source should mention {expected}"
        );
    }
}

#[test]
fn parity_capability_map_records_p249_matrix_bridge_once_puppet_assembly_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let matrix_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-matrix/src/lib.rs"))
            .expect("read agentd-matrix source");
    let bin_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-bin/src/matrix_bridge.rs"))
            .expect("read agentd-bin matrix bridge source");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p249",
        "BridgeOncePuppetAccountConfig",
        "MatrixPuppetTokenFileStore",
        "MatrixPuppetHttpAccountProvisioner",
        "daemon/SDK account provisioning assembly",
        "daemon/SDK service assembly",
        "Matrix media",
        "cutover",
        "rollback",
        "token rotation",
        "service packaging",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p249",
        "BridgeOncePuppetAccountConfig",
        "matrix-bridge-once",
        "Matrix bridge remains partial",
        "daemon/SDK service assembly",
        "service packaging",
        "token rotation",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in [
        "BridgeOncePuppetAccountConfig",
        "MatrixPuppetHttpAccountProvisioner",
        "MatrixPuppetTokenFileStore",
        "run_bridge_once",
    ] {
        assert!(
            matrix_source.contains(expected),
            "agentd-matrix source should mention {expected}"
        );
    }

    for expected in ["matrix_puppet_account_config", "matrix_homeserver_url"] {
        assert!(
            bin_source.contains(expected),
            "agentd-bin matrix bridge source should mention {expected}"
        );
    }
}

#[test]
fn parity_capability_map_records_p250_matrix_sdk_bridge_once_assembly_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let matrix_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-matrix/src/lib.rs"))
            .expect("read agentd-matrix source");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p250",
        "MatrixClientBridgeOnceConfig",
        "run_matrix_client_bridge_once",
        "MatrixClientBridgeTransport",
        "AgentdHttpBackend",
        "BridgeRuntime",
        "daemon service assembly",
        "Matrix media",
        "cutover",
        "rollback",
        "token rotation",
        "service packaging",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p250",
        "MatrixClientBridgeOnceConfig",
        "run_matrix_client_bridge_once",
        "Matrix bridge remains partial",
        "daemon service assembly",
        "service packaging",
        "token rotation",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in [
        "MatrixClientBridgeOnceConfig",
        "run_matrix_client_bridge_once",
        "MatrixClientBridgeTransport",
        "AgentdHttpBackend",
        "BridgeRuntime",
    ] {
        assert!(
            matrix_source.contains(expected),
            "agentd-matrix source should mention {expected}"
        );
    }
}

#[test]
fn parity_capability_map_records_p251_matrix_client_bridge_service_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let bin_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-bin/src/matrix_bridge.rs"))
            .expect("read agentd-bin matrix bridge source");
    let cli_source = std::fs::read_to_string(repo_root().join("crates/agentd-bin/src/cli.rs"))
        .expect("read agentd-bin cli source");
    let cargo_source = std::fs::read_to_string(repo_root().join("crates/agentd-bin/Cargo.toml"))
        .expect("read agentd-bin Cargo.toml");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p251",
        "MatrixClientBridgeServiceConfig",
        "run_matrix_client_bridge_service",
        "matrix-client-bridge-service",
        "real homeserver validation",
        "Matrix media",
        "cutover",
        "rollback",
        "token rotation",
        "service packaging",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p251",
        "MatrixClientBridgeServiceConfig",
        "run_matrix_client_bridge_service",
        "bounded service",
        "Matrix bridge remains partial",
        "real homeserver validation",
        "service packaging",
        "token rotation",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in [
        "MatrixClientBridgeServiceConfig",
        "MatrixClientBridgeServiceReport",
        "run_matrix_client_bridge_service",
        "run_matrix_sdk_bridge_service",
    ] {
        assert!(
            bin_source.contains(expected),
            "agentd-bin matrix bridge source should mention {expected}"
        );
    }

    for expected in [
        "matrix-client-bridge-service",
        "MatrixClientBridgeServiceArgs",
        "MatrixClientBridgeService",
    ] {
        assert!(
            cli_source.contains(expected),
            "agentd-bin CLI source should mention {expected}"
        );
    }

    assert!(
        cargo_source.contains("matrix-sdk-adapter"),
        "agentd-bin Cargo.toml should expose the feature-gated SDK adapter path"
    );
}

#[test]
fn parity_capability_map_records_p252_matrix_client_bridge_preflight_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let bin_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-bin/src/matrix_bridge.rs"))
            .expect("read agentd-bin matrix bridge source");
    let cli_source = std::fs::read_to_string(repo_root().join("crates/agentd-bin/src/cli.rs"))
        .expect("read agentd-bin cli source");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p252",
        "MatrixClientBridgePreflightReport",
        "run_matrix_client_bridge_preflight",
        "matrix-client-bridge-preflight",
        "service packaging",
        "Matrix media",
        "cutover",
        "rollback",
        "token rotation",
        "dashboard/operator visibility",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p252",
        "MatrixClientBridgePreflightReport",
        "run_matrix_client_bridge_preflight",
        "operator preflight",
        "Matrix bridge remains partial",
        "service packaging",
        "token rotation",
        "dashboard/operator visibility",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in [
        "MatrixClientBridgePreflightReport",
        "MatrixHomeserverPreflightReport",
        "run_matrix_client_bridge_preflight",
    ] {
        assert!(
            bin_source.contains(expected),
            "agentd-bin matrix bridge source should mention {expected}"
        );
    }

    for expected in [
        "matrix-client-bridge-preflight",
        "MatrixClientBridgePreflightArgs",
        "MatrixClientBridgePreflight",
    ] {
        assert!(
            cli_source.contains(expected),
            "agentd-bin CLI source should mention {expected}"
        );
    }
}

#[test]
fn parity_capability_map_records_p253_matrix_preflight_smoke_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let script_source = std::fs::read_to_string(
        repo_root().join("scripts/agentd_matrix_client_bridge_preflight_smoke.sh"),
    )
    .expect("read Matrix preflight smoke script");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p253",
        "agentd_matrix_client_bridge_preflight_smoke.sh",
        "AGENTD_REAL_MATRIX_PREFLIGHT_SMOKE",
        "service packaging",
        "Matrix media",
        "cutover",
        "rollback",
        "token rotation",
        "dashboard/operator visibility",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p253",
        "agentd_matrix_client_bridge_preflight_smoke.sh",
        "real-environment Matrix preflight",
        "smoke harness",
        "Matrix bridge remains partial",
        "service packaging",
        "token rotation",
        "dashboard/operator visibility",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in [
        "AGENTD_REAL_MATRIX_PREFLIGHT_SMOKE",
        "matrix-client-bridge-preflight",
        "preflight.out",
        "preflight.err",
        "summary.txt",
        "access_token: set (redacted)",
    ] {
        assert!(
            script_source.contains(expected),
            "Matrix preflight smoke script should mention {expected}"
        );
    }
}

#[test]
fn parity_capability_map_records_p254_matrix_service_smoke_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let script_source = std::fs::read_to_string(
        repo_root().join("scripts/agentd_matrix_client_bridge_service_smoke.sh"),
    )
    .expect("read Matrix service smoke script");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p254",
        "agentd_matrix_client_bridge_service_smoke.sh",
        "AGENTD_REAL_MATRIX_SERVICE_SMOKE",
        "service packaging",
        "Matrix media",
        "cutover",
        "rollback",
        "token rotation",
        "dashboard/operator visibility",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p254",
        "agentd_matrix_client_bridge_service_smoke.sh",
        "bounded service smoke",
        "AGENTD_REAL_MATRIX_SERVICE_SMOKE",
        "Matrix bridge remains partial",
        "service packaging",
        "token rotation",
        "dashboard/operator visibility",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in [
        "AGENTD_REAL_MATRIX_SERVICE_SMOKE",
        "matrix-sdk-adapter",
        "matrix-client-bridge-preflight",
        "matrix-client-bridge-service",
        "service.out",
        "service.err",
        "summary.txt",
        "password: set (redacted)",
    ] {
        assert!(
            script_source.contains(expected),
            "Matrix service smoke script should mention {expected}"
        );
    }
}

#[test]
fn parity_capability_map_records_p255_matrix_bot_command_planner_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let matrix_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-matrix/src/lib.rs"))
            .expect("read agentd-matrix source");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p255",
        "bot command planner",
        "Matrix media",
        "service packaging",
        "cutover",
        "rollback",
        "token rotation",
        "bridge operations",
        "dashboard/operator visibility",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p255",
        "bot command planner",
        "Matrix bridge remains partial",
        "command execution",
        "dashboard/operator visibility",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in [
        "MatrixBotCommandPlan",
        "MatrixBotCommandAcl",
        "Send !help for available commands.",
    ] {
        assert!(
            matrix_source.contains(expected),
            "agentd-matrix source should mention {expected}"
        );
    }
}

#[test]
fn parity_capability_map_records_p256_matrix_bot_command_ingress_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let matrix_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-matrix/src/lib.rs"))
            .expect("read agentd-matrix source");
    let bin_source = std::fs::read_to_string(repo_root().join("crates/agentd-bin/src/cli.rs"))
        .expect("read agentd-bin cli source");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p256",
        "bot command ingress classification",
        "management command execution",
        "Matrix media",
        "service packaging",
        "cutover",
        "rollback",
        "token rotation",
        "bridge operations",
        "dashboard/operator visibility",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p256",
        "bot command ingress classification",
        "--matrix-operator",
        "--matrix-admin",
        "command omission from inbound forwarding",
        "Matrix bridge remains partial",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in ["bot_command_plans", "formatted_body", "MatrixBotCommandAcl"] {
        assert!(
            matrix_source.contains(expected),
            "agentd-matrix source should mention {expected}"
        );
    }
    assert!(
        bin_source.contains("matrix_operator_mxids"),
        "agentd-bin CLI source should mention matrix_operator_mxids"
    );
}

#[test]
fn parity_capability_map_records_p257_matrix_bot_readonly_command_replies_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let matrix_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-matrix/src/lib.rs"))
            .expect("read agentd-matrix source");
    let bin_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-bin/src/matrix_bridge.rs"))
            .expect("read agentd-bin matrix bridge source");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p257",
        "read-only bot command replies",
        "management command execution",
        "room lifecycle parity",
        "Matrix media",
        "real homeserver",
        "service packaging",
        "cutover",
        "rollback",
        "token rotation",
        "bridge operations",
        "dashboard/operator visibility",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p257",
        "read-only bot command replies",
        "bot_command_replies_sent",
        "Matrix bridge remains partial",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in [
        "execute_matrix_bot_command",
        "MatrixBotCommandSnapshot",
        "bot_command_replies_sent",
    ] {
        assert!(
            matrix_source.contains(expected),
            "agentd-matrix source should mention {expected}"
        );
    }
    assert!(
        bin_source.contains("bot_command_replies_sent"),
        "agentd-bin matrix bridge source should mention bot_command_replies_sent"
    );
}

#[test]
fn parity_capability_map_records_p258_matrix_bot_management_effects_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let matrix_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-matrix/src/lib.rs"))
            .expect("read agentd-matrix source");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p258",
        "management command effects",
        "`!dm`",
        "`!identity`",
        "room lifecycle parity",
        "Matrix media",
        "real homeserver",
        "service packaging",
        "cutover",
        "rollback",
        "token rotation",
        "bridge operations",
        "dashboard/operator visibility",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p258",
        "management command effects",
        "`!dm`",
        "`!identity`",
        "Matrix bridge remains partial",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in [
        "MatrixBotCommandBackendEffectPort",
        "MatrixBotCommandRoomEffectPort",
        "execute_matrix_bot_command_with_effects",
        "ensure_human_dm_room",
    ] {
        assert!(
            matrix_source.contains(expected),
            "agentd-matrix source should mention {expected}"
        );
    }
}

#[test]
fn parity_capability_map_records_p259_matrix_identity_persistence_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let store_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-store/src/agent_repo.rs"))
            .expect("read agent repo source");
    let surface_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-surface/src/http.rs"))
            .expect("read surface http source");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p259",
        "daemon identity persistence",
        "`PATCH /api/agents/:name`",
        "runtime_profile.identity",
        "real SDK DM room lifecycle",
        "remaining management commands",
        "Matrix media",
        "service packaging",
        "cutover",
        "rollback",
        "token rotation",
        "bridge operations",
        "dashboard/operator visibility",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p259",
        "daemon identity persistence",
        "`PATCH /api/agents/:name`",
        "Matrix bridge remains partial",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in ["update_agent_identity", "runtime_profile"] {
        assert!(
            store_source.contains(expected),
            "agent repo source should mention {expected}"
        );
    }
    assert!(
        surface_source.contains("patch(update_agent_identity"),
        "surface http route should expose PATCH /api/agents/:name"
    );
}

#[test]
fn parity_capability_map_records_p260_matrix_dm_lifecycle_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let matrix_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-matrix/src/lib.rs"))
            .expect("read agentd-matrix source");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p260",
        "SDK-facing DM room lifecycle",
        "`!dm`",
        "Matrix media",
        "remaining management commands",
        "service packaging",
        "cutover",
        "rollback",
        "token rotation",
        "bridge operations",
        "dashboard/operator visibility",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p260",
        "SDK-facing DM room lifecycle",
        "`!dm`",
        "Matrix bridge remains partial",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in [
        "create_direct_room",
        "room_member_status",
        "invite_user_to_room",
    ] {
        assert!(
            matrix_source.contains(expected),
            "agentd-matrix source should mention {expected}"
        );
    }
}

#[test]
fn parity_capability_map_records_p261_matrix_group_management_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let matrix_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-matrix/src/lib.rs"))
            .expect("read agentd-matrix source");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p261",
        "group management effects",
        "`!mkgroup`",
        "`!addmember`",
        "`!rmember`",
        "`!rmgroup`",
        "`!joingroup`",
        "admin commands",
        "Matrix room cleanup",
        "Matrix media",
        "real homeserver",
        "service packaging",
        "cutover",
        "rollback",
        "token rotation",
        "bridge operations",
        "dashboard/operator visibility",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p261",
        "group management effects",
        "`!mkgroup`",
        "`!addmember`",
        "`!rmember`",
        "`!rmgroup`",
        "Matrix bridge remains partial",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in [
        "create_bot_group",
        "update_bot_group_members",
        "delete_bot_group",
    ] {
        assert!(
            matrix_source.contains(expected),
            "agentd-matrix source should mention {expected}"
        );
    }
}

#[test]
fn parity_capability_map_records_p262_matrix_joingroup_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let matrix_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-matrix/src/lib.rs"))
            .expect("read agentd-matrix source");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p262",
        "`!joingroup`",
        "backend",
        "trusted group-room invite",
        "admin commands",
        "Matrix media",
        "real homeserver",
        "service packaging",
        "cutover",
        "rollback",
        "token rotation",
        "bridge operations",
        "dashboard/operator visibility",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p262",
        "`!joingroup`",
        "trusted group-room invite",
        "Matrix bridge remains partial",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in [
        "ensure_human_group_room",
        "render_matrix_bot_joingroup_reply",
    ] {
        assert!(
            matrix_source.contains(expected),
            "agentd-matrix source should mention {expected}"
        );
    }
}

#[test]
fn parity_audit_reports_required_gaps_from_map() {
    let before_count = count_entries(Path::new(AGENT_CHAT_PATH));
    let out = agentctl(&["parity", "audit", "--agent-chat", AGENT_CHAT_PATH]);
    let after_count = count_entries(Path::new(AGENT_CHAT_PATH));

    assert_eq!(
        out.status.code(),
        Some(1),
        "required gaps should be a gate failure; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("required_total="),
        "stdout includes required summary: {stdout}"
    );
    for row in [
        "messaging_inbox",
        "pool_scheduler",
        "migration_shadow_cutover",
    ] {
        assert!(stdout.contains(row), "stdout names {row}: {stdout}");
    }
    assert_eq!(
        before_count, after_count,
        "audit must not create or delete files in agent-chat"
    );
}

#[test]
fn parity_audit_rejects_missing_agent_chat_path() {
    let out = agentctl(&[
        "parity",
        "audit",
        "--agent-chat",
        "/tmp/agent-chat-path-that-does-not-exist",
    ]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "invalid input exits 2; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("invalid agent-chat path"),
        "stderr explains invalid path: {stderr}"
    );
}
