use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TASK_RUN_ID: &str = "tr_0123456789ABCDEFGHJKMNPQRS";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run(cmd: &mut Command, label: &str) -> Output {
    let output = cmd.output().unwrap_or_else(|err| panic!("{label}: {err}"));
    assert!(
        output.status.success(),
        "{label} failed\nstatus: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn git(dir: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir).args(args);
    run(&mut cmd, &format!("git {}", args.join(" ")));
}

fn init_repo(path: &Path) {
    fs::create_dir(path).expect("create repo");
    git(path, &["init"]);
    git(path, &["config", "user.email", "agentd@example.invalid"]);
    git(path, &["config", "user.name", "agentd test"]);
}

fn commit_file(repo: &Path, file: &str, body: &str, message: &str) {
    fs::write(repo.join(file), body).expect("write file");
    git(repo, &["add", file]);
    git(repo, &["commit", "-m", message]);
}

fn branch_main(repo: &Path) {
    git(repo, &["branch", "-M", "main"]);
}

fn fake_path(fakebin: &Path) -> String {
    let existing = std::env::var("PATH").unwrap_or_default();
    format!("{}:{existing}", fakebin.display())
}

#[cfg(unix)]
fn write_fake_gh(fakebin: &Path) {
    use std::os::unix::fs::PermissionsExt;

    fs::create_dir(fakebin).expect("create fakebin");
    let gh = fakebin.join("gh");
    fs::write(
        &gh,
        r#"#!/usr/bin/env bash
printf '%s\n' "$@" >"$GH_LOG"
echo 'https://example.invalid/pull/1'
"#,
    )
    .expect("write fake gh");
    let mut perms = fs::metadata(&gh).expect("fake gh metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&gh, perms).expect("chmod fake gh");
}

fn assert_no_common_history_guidance(stderr: &str, task_branch: &str, base_branch: &str) {
    let base_ref = format!("origin/{base_branch}");
    let retry = format!("bash scripts/agentd_open_pr.sh {TASK_RUN_ID} {base_branch}");

    assert!(
        stderr.contains("repair task branch with:"),
        "stderr should introduce repair guidance: {stderr}"
    );
    assert!(
        stderr.contains(&format!("git switch {task_branch}")),
        "stderr should switch to the failed task branch: {stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "bash scripts/agentd_pr_history_bridge.sh {base_branch}"
        )),
        "stderr should show the bridge dry-run command: {stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "AGENTD_PR_HISTORY_BRIDGE=1 bash scripts/agentd_pr_history_bridge.sh --execute {base_branch}"
        )),
        "stderr should show the guarded bridge execute command: {stderr}"
    );
    assert!(
        stderr.contains(&format!("git push origin {task_branch}")),
        "stderr should push the repaired task branch normally: {stderr}"
    );
    assert!(
        stderr.contains(&retry),
        "stderr should include a retry command: {stderr}"
    );
    assert!(
        stderr.contains(&base_ref),
        "stderr should name the requested base ref: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn open_pr_rejects_no_common_history_before_gh() {
    let temp = tempfile::tempdir().expect("tempdir");
    let origin_seed = temp.path().join("origin-seed");
    let origin = temp.path().join("origin.git");
    let repo = temp.path().join("repo");
    let fakebin = temp.path().join("bin");
    let gh_log = temp.path().join("gh.log");

    init_repo(&origin_seed);
    commit_file(&origin_seed, "README.md", "remote seed\n", "remote seed");
    branch_main(&origin_seed);
    let mut clone_bare = Command::new("git");
    clone_bare.args([
        "clone",
        "--bare",
        origin_seed.to_str().expect("origin seed path"),
        origin.to_str().expect("origin path"),
    ]);
    run(&mut clone_bare, "git clone --bare");

    init_repo(&repo);
    commit_file(&repo, "README.md", "local seed\n", "local seed");
    branch_main(&repo);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    let task_branch = format!("agentd/{TASK_RUN_ID}");
    git(&repo, &["switch", "-c", &task_branch]);
    git(&repo, &["push", "origin", &task_branch]);
    write_fake_gh(&fakebin);

    let output = Command::new("bash")
        .current_dir(&repo)
        .arg(repo_root().join("scripts/agentd_open_pr.sh"))
        .arg(TASK_RUN_ID)
        .env("PATH", fake_path(&fakebin))
        .env("GH_LOG", &gh_log)
        .output()
        .expect("run open_pr helper");

    assert!(
        !output.status.success(),
        "no-common-history branch must be rejected before gh"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no common history")
            && stderr.contains(&task_branch)
            && stderr.contains("origin/main"),
        "stderr names compared refs: {stderr}"
    );
    assert_no_common_history_guidance(&stderr, &task_branch, "main");
    assert!(
        !gh_log.exists(),
        "gh must not be called on preflight failure"
    );
}

#[cfg(unix)]
#[test]
fn open_pr_no_common_history_guidance_uses_requested_base_branch() {
    let temp = tempfile::tempdir().expect("tempdir");
    let origin_seed = temp.path().join("origin-seed");
    let origin = temp.path().join("origin.git");
    let repo = temp.path().join("repo");
    let fakebin = temp.path().join("bin");
    let gh_log = temp.path().join("gh.log");

    init_repo(&origin_seed);
    commit_file(&origin_seed, "README.md", "remote seed\n", "remote seed");
    git(&origin_seed, &["branch", "-M", "release"]);
    let mut clone_bare = Command::new("git");
    clone_bare.args([
        "clone",
        "--bare",
        origin_seed.to_str().expect("origin seed path"),
        origin.to_str().expect("origin path"),
    ]);
    run(&mut clone_bare, "git clone --bare");

    init_repo(&repo);
    commit_file(&repo, "README.md", "local seed\n", "local seed");
    branch_main(&repo);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    let task_branch = format!("agentd/{TASK_RUN_ID}");
    git(&repo, &["switch", "-c", &task_branch]);
    git(&repo, &["push", "origin", &task_branch]);
    write_fake_gh(&fakebin);

    let output = Command::new("bash")
        .current_dir(&repo)
        .arg(repo_root().join("scripts/agentd_open_pr.sh"))
        .arg(TASK_RUN_ID)
        .arg("release")
        .env("PATH", fake_path(&fakebin))
        .env("GH_LOG", &gh_log)
        .output()
        .expect("run open_pr helper");

    assert!(
        !output.status.success(),
        "no-common-history branch must be rejected before gh"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no common history")
            && stderr.contains(&task_branch)
            && stderr.contains("origin/release"),
        "stderr names compared refs: {stderr}"
    );
    assert_no_common_history_guidance(&stderr, &task_branch, "release");
    assert!(
        !gh_log.exists(),
        "gh must not be called on preflight failure"
    );
}

#[cfg(unix)]
#[test]
fn open_pr_invokes_gh_with_explicit_base_and_head() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let origin = temp.path().join("origin.git");
    let fakebin = temp.path().join("bin");
    let gh_log = temp.path().join("gh.log");
    let task_branch = format!("agentd/{TASK_RUN_ID}");

    init_repo(&repo);
    commit_file(&repo, "README.md", "seed\n", "seed");
    branch_main(&repo);
    let mut init_bare = Command::new("git");
    init_bare.args(["init", "--bare", origin.to_str().unwrap()]);
    run(&mut init_bare, "git init --bare");
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "origin", "main"]);
    git(&repo, &["switch", "-c", &task_branch]);
    commit_file(&repo, "task.txt", "task branch\n", "task branch");
    git(&repo, &["push", "origin", &task_branch]);
    write_fake_gh(&fakebin);

    let output = Command::new("bash")
        .current_dir(&repo)
        .arg(repo_root().join("scripts/agentd_open_pr.sh"))
        .arg(TASK_RUN_ID)
        .env("PATH", fake_path(&fakebin))
        .env("GH_LOG", &gh_log)
        .output()
        .expect("run open_pr helper");

    assert!(
        output.status.success(),
        "compatible history opens PR\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(&gh_log).expect("read gh log"),
        format!("pr\ncreate\n--fill\n--base\nmain\n--head\n{task_branch}\n")
    );
}
