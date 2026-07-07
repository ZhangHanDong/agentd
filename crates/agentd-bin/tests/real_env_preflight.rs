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
        .join("agentd_real_env_preflight.sh")
}

fn run_script(args: &[&str]) -> Output {
    Command::new("bash")
        .arg(script_path())
        .args(args)
        .current_dir(repo_root())
        .output()
        .expect("run real env preflight script")
}

#[test]
fn real_env_preflight_dry_run_prints_plan_without_starting() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_dir = temp.path().join("state");
    let state_dir_arg = state_dir.to_string_lossy().to_string();
    let out = run_script(&[
        "--dry-run",
        "--run-id",
        "env-dry",
        "--state-dir",
        &state_dir_arg,
    ]);

    assert!(
        out.status.success(),
        "dry-run exits 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for expected in [
        "agentd_pr_history_status.sh HEAD main",
        "agentd_real_claude_smoke.sh --preflight-only",
        "agentd_real_execute_smoke.sh --preflight-only",
        "agentd_real_sigkill_smoke.sh --preflight-only",
        "does not run AGENTD_REAL_* --execute",
    ] {
        assert!(
            stdout.contains(expected),
            "plan should contain {expected}: {stdout}"
        );
    }
    assert!(
        !state_dir.exists(),
        "dry-run should not create the state directory"
    );
}

#[test]
fn real_env_preflight_rejects_execute_mode() {
    let out = run_script(&["--execute"]);

    assert!(!out.status.success(), "execute mode should be refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does not run AGENTD_REAL_* --execute"),
        "stderr should name aggregate execute refusal: {stderr}"
    );
}

#[test]
fn real_env_preflight_preflight_only_accepts_fake_prereqs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fakebin = temp.path().join("bin");
    fs::create_dir(&fakebin).expect("fakebin");
    write_fake_preflight_tools(&fakebin, true);

    let state_dir = temp.path().join("state");
    let out = Command::new("bash")
        .arg(script_path())
        .args([
            "--preflight-only",
            "--run-id",
            "env-preflight",
            "--state-dir",
            state_dir.to_string_lossy().as_ref(),
        ])
        .current_dir(repo_root())
        .env("PATH", fake_path(&fakebin))
        .output()
        .expect("run aggregate preflight");

    assert!(
        out.status.success(),
        "fake prereqs pass; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for expected in [
        "[ok] git history",
        "[ok] real Claude preflight",
        "[ok] real execute preflight",
        "[ok] real SIGKILL preflight",
        "real environment preflight ok",
        "no AGENTD_REAL_* --execute commands were run",
    ] {
        assert!(
            stdout.contains(expected),
            "preflight output should contain {expected}: {stdout}"
        );
    }
    assert!(
        !state_dir.join("daemon.log").exists(),
        "aggregate preflight should not start a daemon"
    );
}

#[test]
fn real_env_preflight_history_failure_stops_before_agent_checks() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fakebin = temp.path().join("bin");
    fs::create_dir(&fakebin).expect("fakebin");
    write_fake_preflight_tools(&fakebin, false);

    let out = Command::new("bash")
        .arg(script_path())
        .args(["--preflight-only", "--run-id", "env-history-fail"])
        .current_dir(repo_root())
        .env("PATH", fake_path(&fakebin))
        .output()
        .expect("run aggregate preflight");

    assert!(
        !out.status.success(),
        "history failure should fail aggregate preflight"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[run] git history"), "{stdout}");
    for forbidden in [
        "[ok] real Claude preflight",
        "[ok] real execute preflight",
        "[ok] real SIGKILL preflight",
        "real environment preflight ok",
    ] {
        assert!(
            !stdout.contains(forbidden),
            "history failure should stop before {forbidden}: {stdout}"
        );
    }
}

#[test]
fn real_env_preflight_deployment_checklist_mentions_aggregate_helper() {
    let checklist = fs::read_to_string(repo_root().join("docs/p0.9-deployment-checklist.md"))
        .expect("read checklist");

    assert!(
        checklist.contains("agentd_real_env_preflight.sh --preflight-only"),
        "P0.9 checklist should point to the aggregate safe preflight helper"
    );
    assert!(
        checklist.contains("does not run `AGENTD_REAL_* --execute`"),
        "P0.9 checklist should say aggregate preflight does not run execute gates"
    );
}

fn write_fake_preflight_tools(dir: &Path, has_common_history: bool) {
    write_fake_tool(dir, "cargo", "echo cargo 1.85\n");
    write_fake_tool(dir, "tmux", "echo tmux 3.4\n");
    write_fake_tool(dir, "agent-spec", "echo agent-spec 1.0\n");
    write_fake_tool(dir, "curl", "echo curl 8\n");
    write_fake_tool(dir, "sqlite3", "echo sqlite 3\n");
    write_fake_tool(
        dir,
        "claude",
        "if [[ \"${1:-}\" == \"--help\" ]]; then echo 'Usage: claude --mcp-config cfg'; else echo claude; fi\n",
    );
    write_fake_tool(
        dir,
        "gh",
        "if [[ \"${1:-}\" == \"auth\" && \"${2:-}\" == \"status\" ]]; then exit 0; fi\necho gh\n",
    );
    let merge_base = if has_common_history {
        "merge-base) echo 1111111111111111111111111111111111111111; exit 0 ;;"
    } else {
        "merge-base) echo 'fatal: no merge base' >&2; exit 1 ;;"
    };
    write_fake_tool(
        dir,
        "git",
        &format!(
            r#"case "${{1:-}}" in
  fetch) exit 0 ;;
  rev-parse) echo 1111111111111111111111111111111111111111; exit 0 ;;
  {merge_base}
  *) echo git 2.45 ;;
esac
"#
        ),
    );
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
