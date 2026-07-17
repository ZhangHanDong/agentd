use agentd_store::{SqliteStore, agent_repo};
use serde_json::json;

async fn open_temp() -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("agentd.db");
    let store = SqliteStore::connect(&db).await.expect("connect + migrate");
    (store, dir)
}

fn text(value: &str) -> String {
    value.to_string()
}

#[tokio::test]
async fn agent_registry_registers_lists_and_inspects_agent_chat_identity_fields() {
    let (store, _dir) = open_temp().await;

    let registered = agent_repo::register_agent(
        store.pool(),
        agent_repo::RegisterAgent {
            name: "codex-sec".to_string(),
            role: Some(text("reviewer")),
            capability: Some(text("strong")),
            runtime: Some(text("codex")),
            model: Some(text("gpt-5")),
            native_runtime_ref: Some(text("native://rs_sec/ra_sec")),
            home_dir: Some(text("/tmp/agentd/homes/agents/agent_codex_sec")),
            workdir: Some(text("/tmp/agentd/homes/agents/agent_codex_sec/workdir")),
            state_dir: Some(text("/tmp/agentd/homes/agents/agent_codex_sec/state")),
            server: Some(text("local")),
            runtime_profile: json!({
                "primary": {
                    "framework": "codex",
                    "provider": "openai",
                    "model": "gpt-5"
                }
            }),
        },
    )
    .await
    .expect("register");

    assert_eq!(registered.name, "codex-sec");
    assert_eq!(registered.role.as_deref(), Some("reviewer"));
    assert_eq!(registered.capability.as_deref(), Some("strong"));
    assert_eq!(registered.runtime.as_deref(), Some("codex"));
    assert_eq!(registered.model.as_deref(), Some("gpt-5"));
    assert_eq!(
        registered.native_runtime_ref.as_deref(),
        Some("native://rs_sec/ra_sec")
    );
    assert_eq!(registered.status, "online");
    assert_eq!(registered.runtime_profile["primary"]["framework"], "codex");

    let listed = agent_repo::list_agents(store.pool()).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "codex-sec");

    let inspected = agent_repo::get_agent(store.pool(), "codex-sec")
        .await
        .expect("inspect")
        .expect("agent exists");
    assert_eq!(inspected.name, "codex-sec");
    assert_eq!(
        inspected.workdir.as_deref(),
        Some("/tmp/agentd/homes/agents/agent_codex_sec/workdir")
    );
    assert_eq!(
        inspected.state_dir.as_deref(),
        Some("/tmp/agentd/homes/agents/agent_codex_sec/state")
    );
    assert_eq!(inspected.server.as_deref(), Some("local"));
}

#[tokio::test]
async fn agent_registry_identity_patch_persists_runtime_profile_text() {
    let (store, _dir) = open_temp().await;

    agent_repo::register_agent(
        store.pool(),
        agent_repo::RegisterAgent {
            name: "codex-worker".to_string(),
            role: Some(text("agent")),
            capability: Some(text("coding")),
            runtime: Some(text("codex")),
            model: Some(text("gpt-5")),
            native_runtime_ref: None,
            home_dir: None,
            workdir: Some(text("/tmp/agentd/codex-worker")),
            state_dir: None,
            server: Some(text("local")),
            runtime_profile: json!({
                "primary": {
                    "framework": "codex",
                    "model": "gpt-5"
                }
            }),
        },
    )
    .await
    .expect("register");

    let updated = agent_repo::update_agent_identity(
        store.pool(),
        "codex-worker",
        "Be concise and report blockers",
    )
    .await
    .expect("identity patch")
    .expect("agent exists");

    assert_eq!(
        updated.runtime_profile["identity"],
        "Be concise and report blockers"
    );
    assert_eq!(updated.runtime_profile["primary"]["framework"], "codex");

    let inspected = agent_repo::get_agent(store.pool(), "codex-worker")
        .await
        .expect("inspect")
        .expect("agent exists");
    assert_eq!(
        inspected.runtime_profile["identity"],
        "Be concise and report blockers"
    );
    assert_eq!(inspected.runtime_profile["primary"]["framework"], "codex");
}

#[tokio::test]
async fn agent_registry_identity_patch_rejects_empty_and_unknown_agents() {
    let (store, _dir) = open_temp().await;

    agent_repo::register_agent(
        store.pool(),
        agent_repo::RegisterAgent {
            name: "codex-worker".to_string(),
            role: Some(text("agent")),
            capability: Some(text("coding")),
            runtime: Some(text("codex")),
            model: Some(text("gpt-5")),
            native_runtime_ref: None,
            home_dir: None,
            workdir: Some(text("/tmp/agentd/codex-worker")),
            state_dir: None,
            server: Some(text("local")),
            runtime_profile: json!({ "primary": { "framework": "codex" } }),
        },
    )
    .await
    .expect("register");

    let empty = agent_repo::update_agent_identity(store.pool(), "codex-worker", " \n\t ").await;
    assert!(empty.is_err(), "empty identity must be rejected");

    let inspected = agent_repo::get_agent(store.pool(), "codex-worker")
        .await
        .expect("inspect")
        .expect("agent exists");
    assert!(
        inspected.runtime_profile.get("identity").is_none(),
        "empty patch must not mutate runtime_profile: {:?}",
        inspected.runtime_profile
    );

    let missing = agent_repo::update_agent_identity(store.pool(), "ghost", "Be concise")
        .await
        .expect("unknown update");
    assert!(missing.is_none(), "unknown agent returns None");
}

#[tokio::test]
async fn agent_registry_heartbeat_and_offline_update_liveness_state() {
    let (store, _dir) = open_temp().await;

    let (agent, created) = agent_repo::heartbeat_agent(
        store.pool(),
        "codex-worker",
        agent_repo::HeartbeatAgent {
            server: Some(text("local")),
            native_runtime_ref: Some(text("native://rs_worker/ra_worker")),
            workspace_path: Some(text("/tmp/agentd/homes/agents/agent_codex_worker/workdir")),
        },
    )
    .await
    .expect("heartbeat");

    assert!(created, "heartbeat creates a missing agent");
    assert_eq!(agent.name, "codex-worker");
    assert_eq!(agent.status, "online");
    assert_eq!(agent.server.as_deref(), Some("local"));
    assert_eq!(
        agent.native_runtime_ref.as_deref(),
        Some("native://rs_worker/ra_worker")
    );
    assert_eq!(
        agent.workdir.as_deref(),
        Some("/tmp/agentd/homes/agents/agent_codex_worker/workdir")
    );
    assert!(agent.last_seen_at.is_some(), "heartbeat records liveness");

    let offline = agent_repo::mark_agent_offline(
        store.pool(),
        "codex-worker",
        agent_repo::OfflineAgent {
            reason: Some(text("manual-offline")),
            clear_runtime: true,
        },
    )
    .await
    .expect("offline")
    .expect("agent exists");

    assert_eq!(offline.status, "offline");
    assert_eq!(offline.offline_reason.as_deref(), Some("manual-offline"));
    assert_eq!(offline.native_runtime_ref, None);
}

#[tokio::test]
async fn agent_registry_start_marks_agent_online_and_records_runtime_state() {
    let (store, _dir) = open_temp().await;

    agent_repo::register_agent(
        store.pool(),
        agent_repo::RegisterAgent {
            name: "codex-worker".to_string(),
            role: Some(text("agent")),
            capability: Some(text("coding")),
            runtime: Some(text("codex")),
            model: Some(text("gpt-5")),
            native_runtime_ref: None,
            home_dir: None,
            workdir: Some(text("/tmp/agentd/codex-worker")),
            state_dir: None,
            server: Some(text("local")),
            runtime_profile: json!({
                "primary": {
                    "framework": "codex",
                    "model": "gpt-5",
                    "extraArgs": "--sandbox danger-full-access"
                }
            }),
        },
    )
    .await
    .expect("register offline agent");

    let started = agent_repo::mark_agent_started(
        store.pool(),
        "codex-worker",
        agent_repo::StartedAgent {
            native_runtime_ref: text("native://rs_worker/ra_worker"),
        },
    )
    .await
    .expect("start")
    .expect("agent exists");
    assert_eq!(started.status, "online");
    assert_eq!(
        started.native_runtime_ref.as_deref(),
        Some("native://rs_worker/ra_worker")
    );
    assert_eq!(started.offline_reason, None);
    assert!(started.last_seen_at.is_some(), "start records liveness");

    let runtime = agent_repo::update_agent_runtime(
        store.pool(),
        "codex-worker",
        agent_repo::RuntimeUpdate {
            blocked: Some(true),
            blocked_reason: Some(text("waiting for reviewer")),
            active_now: Some(false),
            active_duration_sec: Some(0),
            idle_duration_sec: Some(12),
            last_runtime_activity_sec: Some(12),
            workspace_path: Some(text("/tmp/agentd/codex-worker")),
            mcp_present: Some(true),
        },
    )
    .await
    .expect("runtime update")
    .expect("agent exists");
    assert_eq!(runtime["agent"], "codex-worker");
    assert_eq!(runtime["blocked"], true);
    assert_eq!(runtime["blockedReason"], "waiting for reviewer");
    assert_eq!(runtime["activeNow"], false);
    assert_eq!(runtime["idleDurationSec"], 12);
    assert_eq!(runtime["workspacePath"], "/tmp/agentd/codex-worker");
    assert_eq!(runtime["mcpPresent"], true);

    let inspected = agent_repo::get_agent(store.pool(), "codex-worker")
        .await
        .expect("inspect")
        .expect("agent exists");
    assert_eq!(inspected.runtime_state["blocked"], true);
    assert_eq!(inspected.runtime_state["mcpPresent"], true);
}

#[tokio::test]
async fn agent_registry_lifecycle_patch_merges_runtime_state() {
    let (store, _dir) = open_temp().await;

    agent_repo::register_agent(
        store.pool(),
        agent_repo::RegisterAgent {
            name: "codex-worker".to_string(),
            role: Some(text("agent")),
            capability: Some(text("coding")),
            runtime: Some(text("codex")),
            model: Some(text("gpt-5")),
            native_runtime_ref: Some(text("native://rs_worker/ra_worker")),
            home_dir: None,
            workdir: Some(text("/tmp/agentd/codex-worker")),
            state_dir: None,
            server: Some(text("local")),
            runtime_profile: json!({ "primary": { "framework": "codex" } }),
        },
    )
    .await
    .expect("register agent");

    agent_repo::update_agent_runtime(
        store.pool(),
        "codex-worker",
        agent_repo::RuntimeUpdate {
            blocked: Some(true),
            blocked_reason: Some(text("waiting for input")),
            active_now: Some(false),
            active_duration_sec: None,
            idle_duration_sec: Some(9),
            last_runtime_activity_sec: Some(9),
            workspace_path: Some(text("/tmp/agentd/codex-worker")),
            mcp_present: Some(true),
        },
    )
    .await
    .expect("runtime update")
    .expect("agent exists");

    let merged = agent_repo::merge_agent_runtime_state(
        store.pool(),
        "codex-worker",
        json!({
            "lifecycle": {
                "state": "down",
                "action": "agent-down-kill",
                "archivePath": "/tmp/agentd/codex-worker/down.log"
            }
        }),
    )
    .await
    .expect("merge lifecycle")
    .expect("agent exists");

    assert_eq!(merged["blocked"], true);
    assert_eq!(merged["blockedReason"], "waiting for input");
    assert_eq!(merged["mcpPresent"], true);
    assert_eq!(merged["lifecycle"]["state"], "down");
    assert_eq!(merged["lifecycle"]["action"], "agent-down-kill");

    let inspected = agent_repo::get_agent(store.pool(), "codex-worker")
        .await
        .expect("inspect")
        .expect("agent exists");
    assert_eq!(inspected.runtime_state["blocked"], true);
    assert_eq!(inspected.runtime_state["lifecycle"]["state"], "down");
}

#[tokio::test]
async fn agent_registry_rejects_empty_agent_name() {
    let (store, _dir) = open_temp().await;

    let err = agent_repo::register_agent(
        store.pool(),
        agent_repo::RegisterAgent {
            name: "   ".to_string(),
            role: None,
            capability: None,
            runtime: None,
            model: None,
            native_runtime_ref: None,
            home_dir: None,
            workdir: None,
            state_dir: None,
            server: None,
            runtime_profile: json!({}),
        },
    )
    .await
    .expect_err("empty names are rejected");
    assert!(
        err.to_string().contains("agent name"),
        "clear validation error: {err}"
    );

    let listed = agent_repo::list_agents(store.pool()).await.expect("list");
    assert!(listed.is_empty(), "no row inserted for invalid name");
}
