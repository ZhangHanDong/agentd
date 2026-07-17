use std::path::PathBuf;

use agentd_bin::enterprise::{EnterpriseControlPlaneConfig, start_enterprise_coordination};
use agentd_bin::{DaemonConfig, EnterpriseDaemonConfig, SecurityRuntimeMode};
use agentd_core::ports::{
    ProjectAuthorityAvailability, ProjectAuthorityHealth, ProjectAuthorityMode,
};
use agentd_core::types::AuthorityKey;
use agentd_store::SqliteStore;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn daemon_config(root: &std::path::Path, specify_url: String) -> DaemonConfig {
    let authorization = root.join("specify.authorization");
    std::fs::write(&authorization, "Bearer workload-token\n").unwrap();
    DaemonConfig {
        security_mode: SecurityRuntimeMode::Enterprise,
        db_path: root.join("agentd.db"),
        port: 0,
        workflows_dir: PathBuf::from("workflows"),
        repo_dir: root.to_path_buf(),
        worktree_base: root.join("worktrees"),
        log_level: "info".to_string(),
        api_token: Some("operator-token".to_string()),
        agent_tokens: vec!["worker=worker-token".to_string()],
        agent_token_mode: "hard".to_string(),
        enterprise: EnterpriseDaemonConfig {
            control_plane_instance_id: Some(
                "ci_01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            ),
            enterprise_region: Some("cn-east".to_string()),
            enterprise_zone: Some("cn-east-a".to_string()),
            control_plane_endpoint: Some("https://agentd.example".to_string()),
            specify_url: Some(specify_url),
            specify_authority_key: Some("specify:corp".to_string()),
            specify_authorization_file: Some(authorization),
            allow_loopback_specify_http: true,
            control_plane_heartbeat_seconds: 10,
            control_plane_lease_seconds: 30,
        },
    }
}

#[tokio::test]
async fn enterprise_startup_checks_specify_and_acquires_fenced_leadership() {
    let server = MockServer::start().await;
    let health = ProjectAuthorityHealth {
        authority_key: AuthorityKey::new("specify:corp").unwrap(),
        mode: ProjectAuthorityMode::Specify,
        availability: ProjectAuthorityAvailability::Available,
        checked_at: 200,
        authority_revision: Some(9),
    };
    Mock::given(method("GET"))
        .and(path("/v1/project-authority/health"))
        .and(header("authorization", "Bearer workload-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(health))
        .mount(&server)
        .await;
    let directory = tempfile::tempdir().unwrap();
    let config = daemon_config(directory.path(), server.uri());
    let store = SqliteStore::connect(&config.db_path).await.unwrap();
    let handle = start_enterprise_coordination(&config, &store)
        .await
        .unwrap()
        .unwrap();
    let leadership = handle.leadership().await.unwrap();
    assert_eq!(1, leadership.term);
    assert_eq!(1, leadership.fencing_token);
    handle.shutdown().await;
}

#[test]
fn enterprise_startup_rejects_missing_explicit_authority_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = daemon_config(directory.path(), "https://specify.example".to_string());
    config.enterprise.specify_authority_key = None;
    assert!(EnterpriseControlPlaneConfig::from_daemon(&config).is_err());
}
