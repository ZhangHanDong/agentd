use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn script_path() -> PathBuf {
    repo_root()
        .join("scripts")
        .join("agentd_pr_history_bridge.sh")
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

fn git_output(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("git {}: {err}", args.join(" ")))
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

fn unrelated_clean_repo() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let origin_seed = temp.path().join("origin-seed");
    let origin = temp.path().join("origin.git");
    let repo = temp.path().join("repo");

    init_repo(&origin_seed);
    commit_file(&origin_seed, "remote.txt", "remote seed\n", "remote seed");
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
    commit_file(&repo, "local.txt", "local seed\n", "local seed");
    branch_main(&repo);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );

    (temp, repo)
}

fn has_merge_base(repo: &Path) -> bool {
    git_output(repo, &["merge-base", "origin/main", "HEAD"])
        .status
        .success()
}

#[test]
fn pr_history_bridge_dry_run_reports_command_without_changing_history() {
    let (_temp, repo) = unrelated_clean_repo();

    let output = Command::new("bash")
        .current_dir(&repo)
        .arg(script_path())
        .output()
        .expect("run PR history bridge helper");

    assert!(
        output.status.success(),
        "dry-run should succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("mode: dry-run"), "{stdout}");
    assert!(stdout.contains("merge_required: yes"), "{stdout}");
    assert!(
        stdout.contains("git merge --allow-unrelated-histories --no-edit origin/main"),
        "{stdout}"
    );
    assert!(
        !has_merge_base(&repo),
        "dry-run must not create a merge-base"
    );
}

#[test]
fn pr_history_bridge_execute_requires_explicit_opt_in() {
    let (_temp, repo) = unrelated_clean_repo();

    let output = Command::new("bash")
        .current_dir(&repo)
        .arg(script_path())
        .arg("--execute")
        .env_remove("AGENTD_PR_HISTORY_BRIDGE")
        .output()
        .expect("run PR history bridge helper");

    assert!(!output.status.success(), "execute without opt-in fails");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AGENTD_PR_HISTORY_BRIDGE=1"),
        "stderr names opt-in env var: {stderr}"
    );
}

#[test]
fn pr_history_bridge_execute_refuses_dirty_worktree() {
    let (_temp, repo) = unrelated_clean_repo();
    fs::write(repo.join("uncommitted.txt"), "dirty\n").expect("write dirty file");

    let output = Command::new("bash")
        .current_dir(&repo)
        .arg(script_path())
        .arg("--execute")
        .env("AGENTD_PR_HISTORY_BRIDGE", "1")
        .output()
        .expect("run PR history bridge helper");

    assert!(!output.status.success(), "dirty worktree fails");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("dirty worktree"),
        "stderr names dirty worktree: {stderr}"
    );
    assert!(
        !has_merge_base(&repo),
        "dirty execute must not create a merge-base"
    );
}

#[test]
fn pr_history_bridge_execute_creates_local_merge_base() {
    let (_temp, repo) = unrelated_clean_repo();

    let output = Command::new("bash")
        .current_dir(&repo)
        .arg(script_path())
        .arg("--execute")
        .env("AGENTD_PR_HISTORY_BRIDGE", "1")
        .output()
        .expect("run PR history bridge helper");

    assert!(
        output.status.success(),
        "execute should create local merge base\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("mode: execute"), "{stdout}");
    assert!(stdout.contains("merge_required: yes"), "{stdout}");
    assert!(stdout.contains("merge_base: "), "{stdout}");
    assert!(has_merge_base(&repo), "execute should create a merge-base");
}
