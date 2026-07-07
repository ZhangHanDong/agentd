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
        .join("agentd_real_claude_smoke.sh")
}

fn run_script(args: &[&str]) -> Output {
    Command::new("bash")
        .arg(script_path())
        .args(args)
        .current_dir(repo_root())
        .env_remove("AGENTD_REAL_CLAUDE_SMOKE")
        .output()
        .expect("run smoke script")
}

#[test]
fn real_claude_smoke_dry_run_prints_plan_without_starting() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_dir = temp.path().join("state");
    let state_dir_arg = state_dir.to_string_lossy().to_string();
    let out = run_script(&[
        "--dry-run",
        "--run-id",
        "smoke-dry-run",
        "--port",
        "19991",
        "--state-dir",
        &state_dir_arg,
    ]);

    assert!(
        out.status.success(),
        "dry-run exits 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("target/debug/agentd"), "{stdout}");
    assert!(
        stdout.contains("target/debug/agentctl run start --flow draft"),
        "{stdout}"
    );
    assert!(
        stdout.contains("http://127.0.0.1:19991/healthz"),
        "{stdout}"
    );
    assert!(stdout.contains(&state_dir_arg), "{stdout}");
    assert!(
        !state_dir.exists(),
        "dry-run should not create the state directory"
    );
}

#[test]
fn real_claude_smoke_execute_requires_explicit_opt_in() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_dir = temp.path().join("state");
    let state_dir_arg = state_dir.to_string_lossy().to_string();
    let out = run_script(&["--execute", "--state-dir", &state_dir_arg]);

    assert!(!out.status.success(), "execute without opt-in fails");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("AGENTD_REAL_CLAUDE_SMOKE=1"),
        "stderr should name the opt-in env var: {stderr}"
    );
}

#[test]
fn real_claude_smoke_preflight_fails_when_tool_is_missing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fakebin = temp.path().join("bin");
    fs::create_dir(&fakebin).expect("fakebin");
    for tool in ["cargo", "tmux", "agent-spec", "curl"] {
        write_fake_tool(&fakebin, tool, "echo ok\n");
    }
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
        .expect("run smoke preflight");

    assert!(!out.status.success(), "missing claude fails preflight");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("claude"),
        "stderr should name the missing claude prerequisite: {stderr}"
    );
}

#[test]
fn real_claude_smoke_preflight_accepts_fake_tools() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fakebin = temp.path().join("bin");
    fs::create_dir(&fakebin).expect("fakebin");
    write_fake_tool(&fakebin, "cargo", "echo cargo 1.85\n");
    write_fake_tool(&fakebin, "tmux", "echo tmux 3.4\n");
    write_fake_tool(&fakebin, "agent-spec", "echo agent-spec 1.0\n");
    write_fake_tool(&fakebin, "curl", "echo curl 8\n");
    write_fake_tool(
        &fakebin,
        "claude",
        "echo 'Usage: claude --mcp-config cfg'\n",
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
        .expect("run smoke preflight");

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
fn real_claude_smoke_script_declares_evidence_artifacts() {
    let body = fs::read_to_string(script_path()).expect("read smoke script");
    for artifact in [
        "issue.md",
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

#[test]
fn real_claude_smoke_execute_exports_claude_auto_trust() {
    let body = fs::read_to_string(script_path()).expect("read smoke script");
    assert!(
        body.contains("AGENTD_CLAUDE_AUTO_TRUST_WORKSPACE=1"),
        "execute mode should opt the daemon into Claude workspace trust handling"
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
