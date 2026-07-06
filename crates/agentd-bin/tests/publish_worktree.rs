use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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

#[test]
fn publish_worktree_writes_local_acceptance_report() {
    let temp = tempfile::tempdir().expect("tempdir");
    let origin = temp.path().join("origin.git");
    let worktree = temp.path().join("worktree");
    let task_run_id = "tr_0123456789ABCDEFGHJKMNPQRS";
    let branch = format!("agentd/{task_run_id}");

    let mut init_bare = Command::new("git");
    init_bare.args(["init", "--bare", origin.to_str().unwrap()]);
    run(&mut init_bare, "git init --bare");

    fs::create_dir(&worktree).expect("create worktree");
    git(&worktree, &["init"]);
    git(
        &worktree,
        &["config", "user.email", "agentd@example.invalid"],
    );
    git(&worktree, &["config", "user.name", "agentd test"]);
    git(
        &worktree,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    fs::write(worktree.join("README.md"), "seed\n").expect("seed readme");
    git(&worktree, &["add", "README.md"]);
    git(&worktree, &["commit", "-m", "seed"]);

    fs::write(worktree.join("agentd-change.txt"), "published by test\n")
        .expect("write task change");
    let script = repo_root().join("scripts/agentd_publish_worktree.sh");
    let mut publish = Command::new("bash");
    publish
        .current_dir(temp.path())
        .arg(script)
        .arg(&worktree)
        .arg(task_run_id);
    let output = run(&mut publish, "agentd_publish_worktree");

    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        branch,
        "publish helper prints the task branch"
    );
    let report_path = temp.path().join(".agentd/run/report.md");
    let report = fs::read_to_string(&report_path)
        .unwrap_or_else(|err| panic!("read {}: {err}", report_path.display()));
    assert!(
        report.contains(task_run_id),
        "report names the task run id: {report}"
    );
    assert!(
        report.contains(&branch),
        "report names the branch: {report}"
    );
}
