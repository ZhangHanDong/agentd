use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn script_path() -> PathBuf {
    repo_root()
        .join("scripts")
        .join("agentd_real_execute_smoke.sh")
}

fn run_script(args: &[&str]) -> Output {
    Command::new("bash")
        .arg(script_path())
        .args(args)
        .current_dir(repo_root())
        .env_remove("AGENTD_REAL_EXECUTE_SMOKE")
        .output()
        .expect("run execute smoke script")
}

#[test]
fn real_execute_smoke_dry_run_prints_plan_without_starting() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_dir = temp.path().join("state");
    let state_dir_arg = state_dir.to_string_lossy().to_string();
    let out = run_script(&[
        "--dry-run",
        "--run-id",
        "execute-dry-run",
        "--port",
        "19992",
        "--state-dir",
        &state_dir_arg,
    ]);

    assert!(
        out.status.success(),
        "dry-run exits 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("scripts/agentd_write_plan.sh"), "{stdout}");
    assert!(stdout.contains("target/debug/agentd"), "{stdout}");
    assert!(
        stdout.contains("target/debug/agentctl run start --flow execute"),
        "{stdout}"
    );
    assert!(
        stdout.contains("http://127.0.0.1:19992/healthz"),
        "{stdout}"
    );
    assert!(stdout.contains(&state_dir_arg), "{stdout}");
    assert!(
        !state_dir.exists(),
        "dry-run should not create the state directory"
    );
}

#[test]
fn real_execute_smoke_execute_requires_explicit_opt_in() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_dir = temp.path().join("state");
    let state_dir_arg = state_dir.to_string_lossy().to_string();
    let out = run_script(&["--execute", "--state-dir", &state_dir_arg]);

    assert!(!out.status.success(), "execute without opt-in fails");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("AGENTD_REAL_EXECUTE_SMOKE=1"),
        "stderr should name the opt-in env var: {stderr}"
    );
}

#[test]
fn real_execute_smoke_preflight_fails_when_tool_is_missing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fakebin = temp.path().join("bin");
    fs::create_dir(&fakebin).expect("fakebin");
    for tool in ["cargo", "tmux", "claude", "agent-spec", "curl", "git"] {
        write_fake_tool(&fakebin, tool, "echo ok\n");
    }
    write_fake_tool(
        &fakebin,
        "claude",
        "if [[ \"${1:-}\" == \"--help\" ]]; then echo 'Usage: claude --mcp-config cfg'; else echo claude; fi\n",
    );
    let state_dir = temp.path().join("state");
    let out = Command::new("bash")
        .arg(script_path())
        .args([
            "--preflight-only",
            "--state-dir",
            state_dir.to_string_lossy().as_ref(),
        ])
        .current_dir(repo_root())
        .env("PATH", fake_path(&fakebin))
        .output()
        .expect("run execute smoke preflight");

    assert!(!out.status.success(), "missing gh fails preflight");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("gh"),
        "stderr should name the missing gh prerequisite: {stderr}"
    );
}

#[test]
fn real_execute_smoke_preflight_accepts_fake_tools() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fakebin = temp.path().join("bin");
    fs::create_dir(&fakebin).expect("fakebin");
    write_fake_tool(&fakebin, "cargo", "echo cargo 1.85\n");
    write_fake_tool(&fakebin, "tmux", "echo tmux 3.4\n");
    write_fake_tool(&fakebin, "agent-spec", "echo agent-spec 1.0\n");
    write_fake_tool(&fakebin, "curl", "echo curl 8\n");
    write_fake_tool(
        &fakebin,
        "git",
        r#"if [[ "${1:-}" == "-C" ]]; then
  shift 2
fi
case "${1:-}" in
  fetch|rev-parse|merge-base) exit 0 ;;
  *) echo git 2.45 ;;
esac
"#,
    );
    write_fake_tool(
        &fakebin,
        "claude",
        "if [[ \"${1:-}\" == \"--help\" ]]; then echo 'Usage: claude --mcp-config cfg'; else echo claude; fi\n",
    );
    write_fake_tool(
        &fakebin,
        "gh",
        "if [[ \"${1:-}\" == \"auth\" && \"${2:-}\" == \"status\" ]]; then exit 0; fi\necho gh\n",
    );

    let state_dir = temp.path().join("state");
    let out = Command::new("bash")
        .arg(script_path())
        .args([
            "--preflight-only",
            "--state-dir",
            state_dir.to_string_lossy().as_ref(),
        ])
        .current_dir(repo_root())
        .env("PATH", fake_path(&fakebin))
        .output()
        .expect("run execute smoke preflight");

    assert!(
        out.status.success(),
        "fake prereqs pass; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("preflight ok"), "{stdout}");
    assert!(
        !state_dir.join("daemon.log").exists(),
        "preflight-only should not start the daemon"
    );
}

#[test]
fn real_execute_smoke_preflight_rejects_no_common_history_before_agents() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fakebin = temp.path().join("bin");
    fs::create_dir(&fakebin).expect("fakebin");
    write_fake_tool(&fakebin, "cargo", "echo cargo 1.85\n");
    write_fake_tool(&fakebin, "tmux", "echo tmux 3.4\n");
    write_fake_tool(&fakebin, "agent-spec", "echo agent-spec 1.0\n");
    write_fake_tool(&fakebin, "curl", "echo curl 8\n");
    write_fake_tool(
        &fakebin,
        "git",
        r#"if [[ "${1:-}" == "-C" ]]; then
  shift 2
fi
case "${1:-}" in
  fetch) exit 0 ;;
  rev-parse) exit 0 ;;
  merge-base) echo 'fatal: no merge base' >&2; exit 1 ;;
  *) echo git 2.45 ;;
esac
"#,
    );
    write_fake_tool(
        &fakebin,
        "claude",
        "if [[ \"${1:-}\" == \"--help\" ]]; then echo 'Usage: claude --mcp-config cfg'; else echo claude; fi\n",
    );
    write_fake_tool(
        &fakebin,
        "gh",
        "if [[ \"${1:-}\" == \"auth\" && \"${2:-}\" == \"status\" ]]; then exit 0; fi\necho gh\n",
    );

    let state_dir = temp.path().join("state");
    let out = Command::new("bash")
        .arg(script_path())
        .args([
            "--preflight-only",
            "--state-dir",
            state_dir.to_string_lossy().as_ref(),
        ])
        .current_dir(repo_root())
        .env("PATH", fake_path(&fakebin))
        .output()
        .expect("run execute smoke preflight");

    assert!(
        !out.status.success(),
        "no common history should fail preflight; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("origin/main") && stderr.contains("HEAD"),
        "stderr should name both refs: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("preflight ok"),
        "failed preflight must not report ok: {stdout}"
    );
    assert!(
        !state_dir.join("daemon.log").exists(),
        "preflight-only should not start the daemon"
    );
}

#[test]
fn real_execute_smoke_dry_run_uses_absolute_daemon_paths() {
    let state_dir = "relative-execute-smoke-state";
    let out = run_script(&[
        "--dry-run",
        "--run-id",
        "execute-absolute-paths",
        "--state-dir",
        state_dir,
    ]);

    assert!(
        out.status.success(),
        "dry-run exits 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let root = repo_root();
    assert!(
        stdout.contains(&format!("--repo-dir '{}'", root.display())),
        "repo-dir is absolute in daemon command: {stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "--worktree-base '{}/{}/worktrees'",
            root.display(),
            state_dir
        )),
        "worktree-base is absolute in daemon command: {stdout}"
    );
}

#[test]
fn real_execute_smoke_dry_run_mentions_history_preflight() {
    let out = run_script(&["--dry-run", "--run-id", "execute-history-preflight"]);

    assert!(
        out.status.success(),
        "dry-run exits 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("origin/main") && stdout.contains("HEAD"),
        "dry-run should document the base-history preflight: {stdout}"
    );
}

#[test]
fn real_execute_smoke_preflight_uses_pr_history_status_helper() {
    let body = fs::read_to_string(script_path()).expect("read execute smoke script");
    assert!(
        body.contains("scripts/agentd_pr_history_status.sh")
            && body.contains("HEAD")
            && body.contains("main"),
        "real execute preflight should reuse the PR history status helper"
    );
}

#[test]
fn real_execute_smoke_script_declares_evidence_artifacts() {
    let body = fs::read_to_string(script_path()).expect("read execute smoke script");
    for artifact in [
        "frozen.spec.md",
        "plan.md",
        "preflight.log",
        "daemon.log",
        "agentctl.out",
        "run_snapshot.json",
        "events.snapshot",
        "summary.txt",
    ] {
        assert!(body.contains(artifact), "script should name {artifact}");
    }
}

fn write_fake_tool(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(
        &path,
        format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}"),
    )
    .expect("write fake tool");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod fake tool");
    }
}

fn fake_path(fakebin: &Path) -> String {
    format!("{}:/bin:/usr/bin", fakebin.display())
}
