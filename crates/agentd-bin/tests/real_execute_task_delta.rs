use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run(cmd: &mut Command, label: &str) -> Output {
    cmd.output().unwrap_or_else(|err| panic!("{label}: {err}"))
}

fn run_ok(cmd: &mut Command, label: &str) -> Output {
    let output = run(cmd, label);
    assert!(
        output.status.success(),
        "{label} failed\nstatus: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn git(dir: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir).args(args);
    run_ok(&mut cmd, &format!("git {}", args.join(" ")))
}

fn seed_repo(path: &Path) -> String {
    fs::create_dir(path).expect("create repository");
    git(path, &["init"]);
    git(path, &["config", "user.email", "agentd@example.invalid"]);
    git(path, &["config", "user.name", "agentd test"]);
    fs::write(path.join("README.md"), "seed\n").expect("write seed");
    git(path, &["add", "README.md"]);
    git(path, &["commit", "-m", "seed"]);
    let head = git(path, &["rev-parse", "HEAD"]);
    String::from_utf8(head.stdout)
        .expect("utf8 head")
        .trim()
        .to_owned()
}

fn verify_delta(worktree: &Path, base: &str) -> Output {
    Command::new("bash")
        .arg(repo_root().join("scripts/agentd_verify_task_delta.sh"))
        .arg(worktree)
        .arg(base)
        .output()
        .expect("run task delta verifier")
}

#[test]
fn real_execute_task_delta_rejects_unchanged_worktree() {
    let temp = tempfile::tempdir().expect("tempdir");
    let worktree = temp.path().join("worktree");
    let base = seed_repo(&worktree);

    let output = verify_delta(&worktree, &base);

    assert!(!output.status.success(), "unchanged worktree must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no task delta relative to") && stderr.contains(&base),
        "stderr explains the missing delta: {stderr}"
    );
}

#[test]
fn real_execute_task_delta_accepts_untracked_change() {
    let temp = tempfile::tempdir().expect("tempdir");
    let worktree = temp.path().join("worktree");
    let base = seed_repo(&worktree);
    fs::write(worktree.join("task-output.txt"), "untracked task output\n")
        .expect("write task output");

    let output = verify_delta(&worktree, &base);

    assert!(
        output.status.success(),
        "untracked task output should pass; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn real_execute_task_delta_accepts_committed_change() {
    let temp = tempfile::tempdir().expect("tempdir");
    let worktree = temp.path().join("worktree");
    let base = seed_repo(&worktree);
    fs::write(worktree.join("task-output.txt"), "committed task output\n")
        .expect("write task output");
    git(&worktree, &["add", "task-output.txt"]);
    git(&worktree, &["commit", "-m", "task output"]);

    let output = verify_delta(&worktree, &base);

    assert!(
        output.status.success(),
        "committed task output should pass; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn real_execute_task_delta_rejects_invalid_base() {
    let temp = tempfile::tempdir().expect("tempdir");
    let worktree = temp.path().join("worktree");
    seed_repo(&worktree);
    let invalid_base = "0000000000000000000000000000000000000000";

    let output = verify_delta(&worktree, invalid_base);

    assert!(!output.status.success(), "invalid base must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid base commit") && stderr.contains(invalid_base),
        "stderr explains the invalid base: {stderr}"
    );
}
