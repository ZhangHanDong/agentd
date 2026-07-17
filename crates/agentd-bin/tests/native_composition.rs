use std::path::Path;
use std::process::Command;

use agentd_bin::DaemonConfig;
use agentd_bin::daemon::build_production_host;

fn git(path: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn production_host_composes_native_runtime_and_runs_startup_recovery() {
    let directory = tempfile::tempdir().expect("native composition directory");
    let repo = directory.path().join("repo");
    std::fs::create_dir_all(&repo).expect("repo");
    git(&repo, &["init"]);
    let config = DaemonConfig {
        security_mode: agentd_bin::SecurityRuntimeMode::Standalone,
        db_path: directory.path().join("state/agentd.db"),
        port: 0,
        workflows_dir: repo.join("workflows"),
        repo_dir: repo.clone(),
        worktree_base: repo.join(".agentd/worktrees"),
        log_level: "error".to_string(),
        api_token: None,
        agent_tokens: Vec::new(),
        agent_token_mode: "audit".to_string(),
    };
    std::fs::create_dir_all(&config.workflows_dir).expect("workflows");

    let host = build_production_host(&config).await.expect("production host");
    assert!(host.native_runtime_service().is_some());
    let manifest = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
    )
    .expect("agentd-bin manifest");
    assert!(!manifest.contains("agentd-tmux"));
}
