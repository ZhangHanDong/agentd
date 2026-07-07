use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn script_path() -> PathBuf {
    repo_root()
        .join("scripts")
        .join("agentd_pr_history_status.sh")
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

fn path_without_gh() -> String {
    "/bin:/usr/bin".to_string()
}

#[test]
fn pr_history_status_reports_no_common_history_without_gh() {
    let temp = tempfile::tempdir().expect("tempdir");
    let origin_seed = temp.path().join("origin-seed");
    let origin = temp.path().join("origin.git");
    let repo = temp.path().join("repo");

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

    let output = Command::new("bash")
        .current_dir(&repo)
        .arg(script_path())
        .env("PATH", path_without_gh())
        .output()
        .expect("run PR history status helper");

    assert!(
        !output.status.success(),
        "unrelated histories should fail the status helper"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("head_ref: HEAD"), "{stdout}");
    assert!(stdout.contains("base_ref: origin/main"), "{stdout}");
    assert!(stdout.contains("merge_base: none"), "{stdout}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no common history")
            && stderr.contains("HEAD")
            && stderr.contains("origin/main"),
        "stderr names compared refs: {stderr}"
    );
}

#[test]
fn pr_history_status_reports_merge_base_for_compatible_history() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let origin = temp.path().join("origin.git");

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
    commit_file(&repo, "task.txt", "task branch\n", "task branch");

    let output = Command::new("bash")
        .current_dir(&repo)
        .arg(script_path())
        .output()
        .expect("run PR history status helper");

    assert!(
        output.status.success(),
        "compatible history should pass\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("head_ref: HEAD"), "{stdout}");
    assert!(stdout.contains("base_ref: origin/main"), "{stdout}");
    assert!(stdout.contains("head_sha: "), "{stdout}");
    assert!(stdout.contains("base_sha: "), "{stdout}");
    assert!(stdout.contains("merge_base: "), "{stdout}");
    assert!(!stdout.contains("merge_base: none"), "{stdout}");
}
