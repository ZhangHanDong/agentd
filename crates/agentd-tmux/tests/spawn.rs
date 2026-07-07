//! Task 2: `AgentBackend::spawn` flow (design §4.5). Test names match the
//! `specs/tmux/p2-spawn-flow.spec.md` selectors. Everything runs against a
//! `RecordingCommandRunner` — no real tmux server.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use agentd_core::CoreError;
use agentd_core::ports::{AgentBackend, CommandError, CommandOutput};
use agentd_core::test_support::RecordingCommandRunner;
use agentd_core::types::{AgentId, CliKind, LaunchStrategy, SpawnRequest};

use agentd_tmux::{Config, TmuxBackend};

const TMUX_BIN: &str = "/opt/homebrew/bin/tmux";

// Wraps in Result to match `RecordingCommandRunner::push_output`'s signature.
#[allow(clippy::unnecessary_wraps)]
fn ok(stdout: &str, status: i32) -> Result<CommandOutput, CommandError> {
    Ok(CommandOutput {
        stdout: stdout.to_string(),
        stderr: String::new(),
        status,
    })
}

fn backend(rec: &Arc<RecordingCommandRunner>) -> TmuxBackend {
    TmuxBackend::new(rec.clone(), TMUX_BIN.into(), Config::default())
}

fn req(worktree: &Path, strategy: LaunchStrategy) -> SpawnRequest {
    SpawnRequest {
        agent_id: AgentId::parsed("claude-impl-a"),
        mxid: None,
        cli: CliKind::ClaudeCode,
        worktree: worktree.to_path_buf(),
        initial_prompt: None,
        env_overrides: HashMap::new(),
        launch_strategy: strategy,
    }
}

fn req_with_mcp_stdio(worktree: &Path, strategy: LaunchStrategy) -> SpawnRequest {
    let mut request = req(worktree, strategy);
    request.env_overrides.insert(
        "AGENTD_MCP_STDIO_CMD".to_string(),
        "agentd --db-path '/tmp/agentd db.sqlite' mcp-stdio".to_string(),
    );
    request
}

#[tokio::test]
async fn spawn_returns_handle_with_parsed_pane_id() {
    let rec = Arc::new(RecordingCommandRunner::new());
    // has-session: non-zero = no such session; new-session: ok; pane probe.
    rec.push_output(ok("", 1));
    rec.push_output(ok("", 0));
    rec.push_output(ok("%3 12345\n", 0));

    let dir = tempfile::tempdir().expect("tempdir");
    let handle = backend(&rec)
        .spawn(req(dir.path(), LaunchStrategy::Direct))
        .await
        .expect("spawn succeeds");

    assert_eq!(handle.session_name, "agentd-claude-impl-a");
    assert_eq!(handle.address, "agentd-claude-impl-a:0.0");
    assert_eq!(handle.pane_id.as_deref(), Some("%3"));
    assert_eq!(handle.pid, Some(12345));

    let calls = rec.calls();
    assert_eq!(calls.len(), 3, "has-session, new-session, display-message");
    assert_eq!(calls[0].args[0], "has-session");
    assert_eq!(calls[0].program, TMUX_BIN);
    assert_eq!(calls[1].program, TMUX_BIN);
    assert_eq!(calls[2].args[0], "display-message");

    // Pin the full Direct launch argv (not just the subcommand) so a dropped
    // -c/-d/-s or a reordered `bash <launcher>` is caught.
    let worktree = dir.path().to_string_lossy().to_string();
    let launcher = dir
        .path()
        .join(".agentd-launcher-claude-impl-a.sh")
        .to_string_lossy()
        .to_string();
    assert_eq!(
        calls[1].args,
        vec![
            "new-session".to_string(),
            "-d".to_string(),
            "-s".to_string(),
            "agentd-claude-impl-a".to_string(),
            "-c".to_string(),
            worktree,
            "bash".to_string(),
            launcher,
        ]
    );
}

#[tokio::test]
async fn spawn_writes_launcher_and_amends_gitignore() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("", 1));
    rec.push_output(ok("", 0));
    rec.push_output(ok("%1 222\n", 0));

    let dir = tempfile::tempdir().expect("tempdir");
    backend(&rec)
        .spawn(req(dir.path(), LaunchStrategy::Direct))
        .await
        .expect("spawn succeeds");

    let launcher = dir.path().join(".agentd-launcher-claude-impl-a.sh");
    assert!(launcher.is_file(), "launcher script should be written");
    let script = std::fs::read_to_string(&launcher).expect("read launcher");
    assert!(script.contains("cd "), "launcher cds into the worktree");
    assert!(
        script.contains("exec claude"),
        "launcher execs the claude CLI: {script}"
    );

    let gitignore =
        std::fs::read_to_string(dir.path().join(".gitignore")).expect("read .gitignore");
    assert!(
        gitignore.contains(".agentd-launcher-*.sh"),
        "gitignore should exclude launcher scripts, got: {gitignore:?}"
    );
}

#[tokio::test]
async fn spawn_launcher_configures_claude_mcp_when_stdio_command_is_present() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("", 1));
    rec.push_output(ok("", 0));
    rec.push_output(ok("%1 222\n", 0));

    let dir = tempfile::tempdir().expect("tempdir");
    backend(&rec)
        .spawn(req_with_mcp_stdio(dir.path(), LaunchStrategy::Direct))
        .await
        .expect("spawn succeeds");

    let mcp_config = dir.path().join(".agentd-mcp-claude-impl-a.json");
    assert!(mcp_config.is_file(), "mcp config should be written");
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&mcp_config).expect("read mcp config"))
            .expect("mcp config is valid json");
    let agentd = &config["mcpServers"]["agentd"];
    assert_eq!(agentd["type"], "stdio");
    assert_eq!(agentd["command"], "sh");
    assert_eq!(agentd["args"][0], "-lc");
    assert_eq!(
        agentd["args"][1],
        "agentd --db-path '/tmp/agentd db.sqlite' mcp-stdio"
    );

    let launcher = dir.path().join(".agentd-launcher-claude-impl-a.sh");
    let script = std::fs::read_to_string(&launcher).expect("read launcher");
    assert!(
        script.contains("exec claude --mcp-config "),
        "launcher should pass --mcp-config: {script}"
    );
    assert!(
        script.contains(".agentd-mcp-claude-impl-a.json"),
        "launcher should point at the per-agent config: {script}"
    );
    assert!(
        script.contains(" --strict-mcp-config"),
        "launcher should restrict Claude to the generated config: {script}"
    );
}

#[tokio::test]
async fn spawn_without_stdio_command_keeps_plain_claude_launcher() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("", 1));
    rec.push_output(ok("", 0));
    rec.push_output(ok("%1 222\n", 0));

    let dir = tempfile::tempdir().expect("tempdir");
    backend(&rec)
        .spawn(req(dir.path(), LaunchStrategy::Direct))
        .await
        .expect("spawn succeeds");

    assert!(
        !dir.path().join(".agentd-mcp-claude-impl-a.json").exists(),
        "plain launcher should not write an MCP config"
    );
    let launcher = dir.path().join(".agentd-launcher-claude-impl-a.sh");
    let script = std::fs::read_to_string(&launcher).expect("read launcher");
    assert!(
        script.contains("exec claude\n"),
        "launcher should keep plain claude exec: {script}"
    );
    assert!(
        !script.contains("--mcp-config"),
        "plain launcher should not pass mcp flags: {script}"
    );
}

#[tokio::test]
async fn spawn_systemd_strategy_wraps_launch() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("", 1));
    rec.push_output(ok("", 0));
    rec.push_output(ok("%9 7\n", 0));

    let dir = tempfile::tempdir().expect("tempdir");
    let strategy = LaunchStrategy::Systemd {
        scope_name: "agentd-claude-impl-a.scope".to_string(),
    };
    backend(&rec)
        .spawn(req(dir.path(), strategy))
        .await
        .expect("spawn succeeds");

    let calls = rec.calls();
    let launch = &calls[1];
    assert_eq!(launch.program, "systemd-run");
    assert!(
        launch.args.contains(&"--scope".to_string()),
        "args: {:?}",
        launch.args
    );
    assert!(
        launch
            .args
            .contains(&"--unit=agentd-claude-impl-a.scope".to_string()),
        "args: {:?}",
        launch.args
    );
    assert!(
        launch.args.contains(&TMUX_BIN.to_string()),
        "args: {:?}",
        launch.args
    );
    assert!(
        launch.args.contains(&"new-session".to_string()),
        "args: {:?}",
        launch.args
    );
}

#[tokio::test]
async fn spawn_on_existing_session_is_recoverable() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("", 0)); // has-session exits 0 → session exists

    let dir = tempfile::tempdir().expect("tempdir");
    let err = backend(&rec)
        .spawn(req(dir.path(), LaunchStrategy::Direct))
        .await
        .expect_err("existing session is an error");

    match err {
        CoreError::Backend(s) => assert!(
            s.contains("rebind"),
            "recoverable error should mention rebinding, got: {s}"
        ),
        other => panic!("expected CoreError::Backend, got {other:?}"),
    }
}

#[tokio::test]
async fn spawn_existing_session_skips_launcher() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("", 0)); // session exists

    let dir = tempfile::tempdir().expect("tempdir");
    let _ = backend(&rec)
        .spawn(req(dir.path(), LaunchStrategy::Direct))
        .await;

    let launcher = dir.path().join(".agentd-launcher-claude-impl-a.sh");
    assert!(
        !launcher.exists(),
        "no launcher when the session already exists"
    );
    assert_eq!(rec.calls().len(), 1, "only the has-session probe ran");
}

#[tokio::test]
async fn spawn_existing_session_skips_mcp_config_artifacts() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("", 0)); // session exists

    let dir = tempfile::tempdir().expect("tempdir");
    let _ = backend(&rec)
        .spawn(req_with_mcp_stdio(dir.path(), LaunchStrategy::Direct))
        .await;

    assert!(
        !dir.path()
            .join(".agentd-launcher-claude-impl-a.sh")
            .exists(),
        "no launcher when the session already exists"
    );
    assert!(
        !dir.path().join(".agentd-mcp-claude-impl-a.json").exists(),
        "no mcp config when the session already exists"
    );
    assert_eq!(rec.calls().len(), 1, "only the has-session probe ran");
}

#[tokio::test]
async fn spawn_unparseable_pane_info_is_error() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("", 1));
    rec.push_output(ok("", 0));
    rec.push_output(ok("   \n", 0)); // empty pane probe → no pane_id

    let dir = tempfile::tempdir().expect("tempdir");
    let err = backend(&rec)
        .spawn(req(dir.path(), LaunchStrategy::Direct))
        .await
        .expect_err("empty pane probe is an error");
    assert!(matches!(err, CoreError::Backend(_)));
}

#[tokio::test]
async fn spawn_handle_has_no_pid_when_probe_omits_it() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("", 1));
    rec.push_output(ok("", 0));
    rec.push_output(ok("%7\n", 0)); // pane_id only, no pid token

    let dir = tempfile::tempdir().expect("tempdir");
    let handle = backend(&rec)
        .spawn(req(dir.path(), LaunchStrategy::Direct))
        .await
        .expect("spawn succeeds");
    assert_eq!(handle.pane_id.as_deref(), Some("%7"));
    assert_eq!(handle.pid, None);
}

#[tokio::test]
async fn spawn_surfaces_launch_failure() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("", 1)); // has-session: none
    rec.push_output(ok("boom", 2)); // new-session fails

    let dir = tempfile::tempdir().expect("tempdir");
    let err = backend(&rec)
        .spawn(req(dir.path(), LaunchStrategy::Direct))
        .await
        .expect_err("a non-zero new-session is an error");
    assert!(matches!(err, CoreError::Backend(_)));
    // Stopped after the failed launch — the pane was never probed.
    assert_eq!(
        rec.calls().len(),
        2,
        "has-session + failed new-session only"
    );
}

#[tokio::test]
async fn spawn_launcher_exports_env_overrides() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("", 1));
    rec.push_output(ok("", 0));
    rec.push_output(ok("%1 5\n", 0));

    let dir = tempfile::tempdir().expect("tempdir");
    let mut request = req(dir.path(), LaunchStrategy::Direct);
    request
        .env_overrides
        .insert("AGENTD_ROLE".to_string(), "needs 'quote'".to_string());
    backend(&rec).spawn(request).await.expect("spawn succeeds");

    let script = std::fs::read_to_string(dir.path().join(".agentd-launcher-claude-impl-a.sh"))
        .expect("read launcher");
    assert!(
        script.contains("export AGENTD_ROLE="),
        "launcher exports the override: {script}"
    );
}

#[tokio::test]
async fn spawn_rejects_invalid_env_key() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("", 1)); // has-session: none, so we reach launcher building

    let dir = tempfile::tempdir().expect("tempdir");
    let mut request = req(dir.path(), LaunchStrategy::Direct);
    request
        .env_overrides
        .insert("BAD KEY".to_string(), "x".to_string());
    let err = backend(&rec)
        .spawn(request)
        .await
        .expect_err("an env key that is not a shell identifier is rejected");
    assert!(matches!(err, CoreError::Backend(_)));
    assert_eq!(rec.calls().len(), 1, "rejected before any launch");
}

#[tokio::test]
async fn spawn_twice_amends_gitignore_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    for pane in ["%1 1\n", "%2 2\n"] {
        let rec = Arc::new(RecordingCommandRunner::new());
        rec.push_output(ok("", 1));
        rec.push_output(ok("", 0));
        rec.push_output(ok(pane, 0));
        backend(&rec)
            .spawn(req(dir.path(), LaunchStrategy::Direct))
            .await
            .expect("spawn succeeds");
    }
    let gitignore =
        std::fs::read_to_string(dir.path().join(".gitignore")).expect("read .gitignore");
    let count = gitignore
        .lines()
        .filter(|line| line.trim() == ".agentd-launcher-*.sh")
        .count();
    assert_eq!(count, 1, "pattern written exactly once: {gitignore:?}");
}

#[tokio::test]
async fn spawn_gitignore_excludes_launcher_and_mcp_config_artifacts() {
    let dir = tempfile::tempdir().expect("tempdir");
    for pane in ["%1 1\n", "%2 2\n"] {
        let rec = Arc::new(RecordingCommandRunner::new());
        rec.push_output(ok("", 1));
        rec.push_output(ok("", 0));
        rec.push_output(ok(pane, 0));
        backend(&rec)
            .spawn(req_with_mcp_stdio(dir.path(), LaunchStrategy::Direct))
            .await
            .expect("spawn succeeds");
    }

    let gitignore =
        std::fs::read_to_string(dir.path().join(".gitignore")).expect("read .gitignore");
    let launcher_count = gitignore
        .lines()
        .filter(|line| line.trim() == ".agentd-launcher-*.sh")
        .count();
    let mcp_config_count = gitignore
        .lines()
        .filter(|line| line.trim() == ".agentd-mcp-*.json")
        .count();
    assert_eq!(
        launcher_count, 1,
        "launcher pattern written exactly once: {gitignore:?}"
    );
    assert_eq!(
        mcp_config_count, 1,
        "mcp config pattern written exactly once: {gitignore:?}"
    );
}

#[tokio::test]
async fn spawn_launcher_execs_codex_for_codex_cli() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("", 1));
    rec.push_output(ok("", 0));
    rec.push_output(ok("%1 1\n", 0));

    let dir = tempfile::tempdir().expect("tempdir");
    let mut request = req(dir.path(), LaunchStrategy::Direct);
    request.cli = CliKind::Codex;
    backend(&rec).spawn(request).await.expect("spawn succeeds");

    let script = std::fs::read_to_string(dir.path().join(".agentd-launcher-claude-impl-a.sh"))
        .expect("read launcher");
    assert!(
        script.contains("exec codex"),
        "codex CLI launcher: {script}"
    );
}
