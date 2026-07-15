use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run_ok(command: &mut Command, label: &str) {
    let output = command
        .output()
        .unwrap_or_else(|err| panic!("{label}: {err}"));
    assert!(
        output.status.success(),
        "{label} failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_executable(path: &Path, body: &str) {
    fs::write(
        path,
        format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}"),
    )
    .expect("write fake executable");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = fs::metadata(path).expect("fake metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod fake executable");
    }
}

fn run_guard(root: &Path, fakebin: &Path, log: &Path, fail: bool) -> Output {
    Command::new("bash")
        .arg(repo_root().join("scripts/agentd_guard_changed_contract.sh"))
        .arg("--staged")
        .current_dir(root)
        .env("PATH", format!("{}:/usr/bin:/bin", fakebin.display()))
        .env("AGENTD_GUARD_LOG", log)
        .env("FAKE_GUARD_FAIL", if fail { "1" } else { "0" })
        .output()
        .expect("run changed-contract guard")
}

fn run_range_guard(root: &Path, fakebin: &Path, log: &Path, base: &str) -> Output {
    Command::new("bash")
        .arg(repo_root().join("scripts/agentd_guard_changed_contract.sh"))
        .args(["--range", base])
        .current_dir(root)
        .env("PATH", format!("{}:/usr/bin:/bin", fakebin.display()))
        .env("AGENTD_GUARD_LOG", log)
        .env("FAKE_GUARD_FAIL", "0")
        .output()
        .expect("run range changed-contract guard")
}

fn initialize_repository(root: &Path) -> String {
    run_ok(
        Command::new("git").arg("init").current_dir(root),
        "git init",
    );
    run_ok(
        Command::new("git")
            .args(["config", "user.email", "guard@example.invalid"])
            .current_dir(root),
        "configure git email",
    );
    run_ok(
        Command::new("git")
            .args(["config", "user.name", "Guard Test"])
            .current_dir(root),
        "configure git name",
    );
    fs::write(root.join("README.md"), "fixture\n").expect("write baseline");
    fs::write(root.join("removed.rs"), "fn removed() {}\n").expect("write removed fixture");
    fs::write(root.join("renamed_before.rs"), "fn renamed() {}\n").expect("write rename fixture");
    run_ok(
        Command::new("git")
            .args(["add", "README.md", "removed.rs", "renamed_before.rs"])
            .current_dir(root),
        "stage baseline",
    );
    run_ok(
        Command::new("git")
            .args(["commit", "-m", "baseline"])
            .current_dir(root),
        "commit baseline",
    );
    let baseline = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .expect("read baseline commit");
    assert!(baseline.status.success(), "read baseline commit");
    String::from_utf8(baseline.stdout)
        .expect("baseline commit is utf-8")
        .trim()
        .to_string()
}

fn stage_guard_task(root: &Path) {
    fs::create_dir_all(root.join("specs/e2e")).expect("create specs");
    fs::write(root.join("specs/e2e/task.spec.md"), "spec: task\n").expect("write candidate spec");
    fs::write(
        root.join("specs/e2e/p156-portable-protected-checks.spec.md"),
        "spec: task\n",
    )
    .expect("write adoption contract");
    fs::write(root.join("changed.rs"), "fn changed() {}\n").expect("write task change");
    fs::remove_file(root.join("removed.rs")).expect("remove tracked fixture");
    fs::rename(
        root.join("renamed_before.rs"),
        root.join("renamed_after.rs"),
    )
    .expect("rename tracked fixture");
    run_ok(
        Command::new("git").args(["add", "-A"]).current_dir(root),
        "stage task",
    );
}

fn install_fake_tools(root: &Path) -> PathBuf {
    let fakebin = root.join("fakebin");
    fs::create_dir_all(&fakebin).expect("create fakebin");
    write_executable(
        &fakebin.join("agent-spec"),
        r#"case "${1:-}" in
  parse) printf '{"meta":{"tags":[]}}\n' ;;
  lifecycle)
    printf '%s\n' "$*" >>"${AGENTD_GUARD_LOG:?}"
    [[ "${FAKE_GUARD_FAIL:-0}" != "1" ]]
    ;;
  *) exit 64 ;;
esac
        "#,
    );
    write_executable(&fakebin.join("jq"), "printf 'lifecycle\\n'\n");
    fakebin
}

fn commit_range_followup(root: &Path) {
    run_ok(
        Command::new("git")
            .args(["commit", "-m", "adopt changed-contract guard"])
            .current_dir(root),
        "commit adoption task",
    );
    fs::write(root.join("followup.rs"), "fn followup() {}\n").expect("write followup");
    run_ok(
        Command::new("git")
            .args(["add", "followup.rs"])
            .current_dir(root),
        "stage followup",
    );
    run_ok(
        Command::new("git")
            .args(["commit", "-m", "followup"])
            .current_dir(root),
        "commit followup",
    );
}

#[test]
fn changed_contract_guard_passes_changes_and_propagates_failure() {
    let fixture = tempfile::tempdir().expect("fixture repository");
    let root = fixture.path();
    let baseline = initialize_repository(root);
    stage_guard_task(root);
    let fakebin = install_fake_tools(root);
    let log = root.join("guard.log");

    let pass = run_guard(root, &fakebin, &log, false);
    assert!(
        pass.status.success(),
        "guard passes; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&pass.stdout),
        String::from_utf8_lossy(&pass.stderr)
    );
    let invocation = fs::read_to_string(&log).expect("read lifecycle invocation");
    for expected in [
        "--change changed.rs",
        "--change removed.rs",
        "--change renamed_after.rs",
        "--change renamed_before.rs",
        "--change specs/e2e/p156-portable-protected-checks.spec.md",
        "--change specs/e2e/task.spec.md",
    ] {
        assert!(
            invocation.contains(expected),
            "missing {expected}: {invocation}"
        );
    }

    let fail = run_guard(root, &fakebin, &log, true);
    assert!(!fail.status.success(), "lifecycle failure must fail guard");

    commit_range_followup(root);
    let range = run_range_guard(root, &fakebin, &log, &baseline);
    assert!(
        range.status.success(),
        "range guard passes; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&range.stdout),
        String::from_utf8_lossy(&range.stderr)
    );
    let invocation = fs::read_to_string(&log).expect("read range lifecycle invocation");
    assert!(
        invocation.contains("--change followup.rs"),
        "range guard must include commits after adoption: {invocation}"
    );
}
