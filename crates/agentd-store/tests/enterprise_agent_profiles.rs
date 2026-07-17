use agentd_core::types::{AgentProfileId, AgentProfileStatus};
use agentd_store::agent_profile_repo::{self, AgentProfileCreate};
use agentd_store::agent_repo::{self, RegisterAgent};
use agentd_store::{SqliteStore, StoreError};
use serde_json::json;

async fn store() -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    (store, dir)
}

fn profile(id: AgentProfileId) -> AgentProfileCreate {
    AgentProfileCreate {
        id,
        role: "implementer".to_string(),
        capability: Some("implementation".to_string()),
        runtime: "codex".to_string(),
        model: Some("gpt-5".to_string()),
        prompt_profile: Some("default".to_string()),
    }
}

fn legacy_agent(id: &str) -> RegisterAgent {
    RegisterAgent {
        name: id.to_string(),
        role: Some("implementer".to_string()),
        capability: Some("implementation".to_string()),
        runtime: Some("codex".to_string()),
        model: Some("gpt-5".to_string()),
        native_runtime_ref: Some("native://rs_profile/ra_profile".to_string()),
        home_dir: None,
        workdir: Some("/tmp/legacy-work".to_string()),
        state_dir: None,
        server: Some("legacy-host".to_string()),
        runtime_profile: json!({"session_name": "legacy-session"}),
    }
}

#[tokio::test]
async fn agent_profile_round_trip_alias_and_lifecycle() {
    let (store, _dir) = store().await;
    let legacy = agent_repo::register_agent(store.pool(), legacy_agent("codex-legacy"))
        .await
        .expect("legacy agent");
    let id = AgentProfileId::new();

    let created = agent_profile_repo::create_profile(store.pool(), profile(id.clone()))
        .await
        .expect("create profile");
    assert_eq!(created.id, id);
    assert_eq!(created.status, AgentProfileStatus::Active);
    assert_eq!(created.record_version, 1);
    assert_eq!(created.role, "implementer");
    assert_eq!(created.runtime, "codex");

    agent_profile_repo::map_legacy_agent(store.pool(), &legacy.id, &id)
        .await
        .expect("map legacy alias");
    assert_eq!(
        agent_profile_repo::profile_for_legacy_agent(store.pool(), &legacy.id)
            .await
            .expect("lookup alias"),
        Some(id.clone())
    );

    let disabled = agent_profile_repo::transition_profile_status(
        store.pool(),
        &id,
        AgentProfileStatus::Disabled,
    )
    .await
    .expect("disable");
    assert_eq!(disabled.status, AgentProfileStatus::Disabled);
    assert_eq!(disabled.record_version, 2);
    let active = agent_profile_repo::transition_profile_status(
        store.pool(),
        &id,
        AgentProfileStatus::Active,
    )
    .await
    .expect("reactivate");
    assert_eq!(active.status, AgentProfileStatus::Active);
    assert_eq!(active.record_version, 3);

    let loaded = agent_profile_repo::get_profile(store.pool(), &id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(loaded, active);
    assert_eq!(
        legacy.native_runtime_ref.as_deref(),
        Some("native://rs_profile/ra_profile")
    );
    assert_eq!(legacy.runtime_profile["session_name"], "legacy-session");
}

#[tokio::test]
async fn agent_profile_rejects_invalid_id_and_retired_reactivation() {
    let (store, _dir) = store().await;
    for invalid in [
        AgentProfileId::from_string("legacy-agent"),
        AgentProfileId::from_string("ap_01ARZ3NDEKTSV4RRFFQ69G5FAI"),
    ] {
        let error = agent_profile_repo::create_profile(store.pool(), profile(invalid))
            .await
            .expect_err("invalid profile id");
        assert!(matches!(error, StoreError::Invariant(_)), "got {error:?}");
    }

    let id = AgentProfileId::new();
    agent_profile_repo::create_profile(store.pool(), profile(id.clone()))
        .await
        .expect("create");
    let retired = agent_profile_repo::transition_profile_status(
        store.pool(),
        &id,
        AgentProfileStatus::Retired,
    )
    .await
    .expect("retire");
    assert_eq!(retired.record_version, 2);

    let error = agent_profile_repo::transition_profile_status(
        store.pool(),
        &id,
        AgentProfileStatus::Active,
    )
    .await
    .expect_err("retired profile must not reactivate");
    assert!(matches!(error, StoreError::Conflict(_)), "got {error:?}");

    let unchanged = agent_profile_repo::get_profile(store.pool(), &id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(unchanged.status, AgentProfileStatus::Retired);
    assert_eq!(unchanged.record_version, 2);
}
