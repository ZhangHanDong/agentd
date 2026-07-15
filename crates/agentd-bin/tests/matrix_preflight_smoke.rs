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
        .join("agentd_matrix_client_bridge_preflight_smoke.sh")
}

fn run_script(args: &[&str]) -> Output {
    run_script_with_env(args, &[])
}

fn run_script_with_env(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new("bash");
    command
        .arg(script_path())
        .args(args)
        .current_dir(repo_root())
        .env_remove("AGENTD_REAL_MATRIX_PREFLIGHT_SMOKE")
        .env_remove("AGENTD_MATRIX_HOMESERVER_URL")
        .env_remove("AGENTD_MATRIX_ACCESS_TOKEN")
        .env_remove("AGENTD_MATRIX_USER_ID")
        .env_remove("AGENTD_MATRIX_DEVICE_ID");
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("run Matrix preflight smoke script")
}

#[test]
fn matrix_preflight_smoke_dry_run_prints_plan_without_side_effects() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_dir = temp.path().join("state");
    let state_dir_arg = state_dir.to_string_lossy().to_string();
    let out = run_script(&[
        "--dry-run",
        "--run-id",
        "matrix-preflight-dry-run",
        "--state-dir",
        &state_dir_arg,
        "--matrix-homeserver-url",
        "http://matrix.example",
        "--matrix-access-token",
        "super-secret-token",
    ]);

    assert!(
        out.status.success(),
        "dry-run exits 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("agentd matrix-client-bridge-preflight"),
        "{stdout}"
    );
    for artifact in ["preflight.out", "preflight.err", "summary.txt"] {
        assert!(
            stdout.contains(artifact),
            "stdout should mention {artifact}: {stdout}"
        );
    }
    assert!(stdout.contains("access_token: set (redacted)"), "{stdout}");
    assert!(
        !stdout.contains("super-secret-token"),
        "dry-run must not print token value: {stdout}"
    );
    assert!(
        !state_dir.exists(),
        "dry-run should not create the state directory"
    );
}

#[test]
fn matrix_preflight_smoke_execute_requires_explicit_opt_in() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_dir = temp.path().join("state");
    let fake_agentd = temp.path().join("agentd");
    write_fake_tool(&fake_agentd, "echo unused\n");
    let out = run_script(&[
        "--execute",
        "--skip-build",
        "--agentd-bin",
        fake_agentd.to_string_lossy().as_ref(),
        "--state-dir",
        state_dir.to_string_lossy().as_ref(),
        "--matrix-homeserver-url",
        "http://matrix.example",
    ]);

    assert!(!out.status.success(), "execute without opt-in fails");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("AGENTD_REAL_MATRIX_PREFLIGHT_SMOKE=1"),
        "stderr should name the opt-in env var: {stderr}"
    );
    assert!(
        !state_dir.exists(),
        "execute without opt-in should not create the state directory"
    );
}

#[test]
fn matrix_preflight_smoke_preflight_only_requires_homeserver_url() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_dir = temp.path().join("state");
    let fake_agentd = temp.path().join("agentd");
    write_fake_tool(&fake_agentd, "echo unused\n");
    let out = run_script(&[
        "--preflight-only",
        "--skip-build",
        "--agentd-bin",
        fake_agentd.to_string_lossy().as_ref(),
        "--state-dir",
        state_dir.to_string_lossy().as_ref(),
    ]);

    assert!(!out.status.success(), "missing homeserver URL fails");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("AGENTD_MATRIX_HOMESERVER_URL"),
        "stderr should name the homeserver env var: {stderr}"
    );
    assert!(
        !state_dir.exists(),
        "preflight-only should not create the state directory"
    );
}

#[test]
fn matrix_preflight_smoke_execute_invokes_agentd_preflight_and_writes_evidence() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fake_agentd = temp.path().join("agentd");
    let args_file = temp.path().join("agentd.args");
    write_fake_tool(
        &fake_agentd,
        r#"printf '%s\n' "$@" > "$FAKE_AGENTD_ARGS"
if [[ "${1:-}" != "matrix-client-bridge-preflight" ]]; then
  echo "unexpected subcommand: ${1:-}" >&2
  exit 7
fi
echo 'matrix-client-bridge-preflight: homeserver=http://matrix.test versions=v1.12 whoami_user_id=@agentd-bot:matrix.test iterations=1 puppet_accounts_configured=false'
"#,
    );
    let state_dir = temp.path().join("state");
    let out = run_script_with_env(
        &[
            "--execute",
            "--skip-build",
            "--agentd-bin",
            fake_agentd.to_string_lossy().as_ref(),
            "--run-id",
            "matrix-preflight-execute",
            "--state-dir",
            state_dir.to_string_lossy().as_ref(),
            "--matrix-homeserver-url",
            "http://matrix.test",
            "--matrix-access-token",
            "top-secret-token",
            "--matrix-user-id",
            "@agentd-bot:matrix.test",
            "--matrix-device-id",
            "DEVICE",
        ],
        &[
            ("AGENTD_REAL_MATRIX_PREFLIGHT_SMOKE", "1"),
            ("FAKE_AGENTD_ARGS", args_file.to_string_lossy().as_ref()),
        ],
    );

    assert!(
        out.status.success(),
        "fake execute succeeds; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let args = fs::read_to_string(&args_file).expect("read fake agentd args");
    assert!(
        args.contains("matrix-client-bridge-preflight"),
        "fake agentd should run the preflight subcommand: {args}"
    );
    assert!(
        args.contains("--matrix-homeserver-url") && args.contains("http://matrix.test"),
        "fake agentd should receive the homeserver URL: {args}"
    );
    assert!(
        args.contains("--matrix-access-token") && args.contains("top-secret-token"),
        "fake agentd should receive the access token flag: {args}"
    );

    let preflight_out =
        fs::read_to_string(state_dir.join("preflight.out")).expect("read preflight.out");
    assert!(
        preflight_out.contains("matrix-client-bridge-preflight: homeserver=http://matrix.test"),
        "{preflight_out}"
    );
    assert!(
        state_dir.join("preflight.err").exists(),
        "preflight.err should be captured even when empty"
    );
    let summary = fs::read_to_string(state_dir.join("summary.txt")).expect("read summary");
    assert!(summary.contains("result: finished"), "{summary}");
    assert!(
        summary.contains("access_token: set (redacted)"),
        "{summary}"
    );
    assert!(
        !summary.contains("top-secret-token"),
        "summary must not print token value: {summary}"
    );
    assert!(
        !state_dir.join("matrix-client-bridge-state.json").exists(),
        "preflight smoke should verify no cursor state was created"
    );
    assert!(
        !state_dir.join("daemon.log").exists(),
        "preflight smoke should not start the daemon"
    );
}

fn write_fake_tool(path: &Path, body: &str) {
    fs::write(
        path,
        format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}"),
    )
    .expect("write fake tool");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod fake tool");
    }
}
