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
    run_script_with_env(args, &[])
}

fn run_script_with_env(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new("bash");
    command
        .arg(script_path())
        .args(args)
        .current_dir(repo_root())
        .env_remove("AGENTD_REAL_EXECUTE_SMOKE")
        .env_remove("AGENTD_REAL_EXECUTE_RUNTIMES");
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("run execute smoke script")
}

fn run_script_in(script: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new("bash")
        .arg(script)
        .args(args)
        .current_dir(cwd)
        .env_remove("AGENTD_REAL_EXECUTE_SMOKE")
        .env_remove("AGENTD_REAL_EXECUTE_RUNTIMES")
        .output()
        .expect("run execute smoke script in fixture repository")
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

fn script_command() -> Command {
    let mut command = Command::new("bash");
    command
        .arg(script_path())
        .current_dir(repo_root())
        .env_remove("AGENTD_REAL_EXECUTE_SMOKE")
        .env_remove("AGENTD_REAL_EXECUTE_RUNTIMES");
    command
}

fn write_fake_execute_preflight_tools(fakebin: &Path, include_claude: bool) {
    write_fake_tool(fakebin, "cargo", "echo cargo 1.85\n");
    write_fake_tool(fakebin, "tmux", "echo tmux 3.4\n");
    write_fake_tool(fakebin, "codex", "echo codex 1.0\n");
    write_fake_tool(fakebin, "agent-spec", "echo agent-spec 1.0\n");
    write_fake_tool(fakebin, "curl", "echo curl 8\n");
    write_fake_tool(
        fakebin,
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
    if include_claude {
        write_fake_tool(
            fakebin,
            "claude",
            "if [[ \"${1:-}\" == \"--help\" ]]; then echo 'Usage: claude --mcp-config cfg'; else echo claude; fi\n",
        );
    }
    write_fake_tool(
        fakebin,
        "gh",
        "if [[ \"${1:-}\" == \"auth\" && \"${2:-}\" == \"status\" ]]; then exit 0; fi\necho gh\n",
    );
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
fn real_execute_smoke_dry_run_prints_run_unique_contract() {
    let out = run_script(&["--dry-run", "--run-id", "p153-contract-01"]);

    assert!(
        out.status.success(),
        "dry-run exits 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for expected in [
        "docs/real-execute-smoke/p153-contract-01.md",
        "crates/agentd-bin/tests/real_execute_smoke_p153_contract_01.rs",
        "AGENTD_REAL_EXECUTE_SMOKE_READY:p153-contract-01",
        "verify_task_delta",
    ] {
        assert!(
            stdout.contains(expected),
            "dry-run should print {expected}: {stdout}"
        );
    }
}

#[test]
fn real_execute_smoke_prepare_only_renders_isolated_contract() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_dir = temp.path().join("state");
    let fakebin = temp.path().join("fakebin");
    fs::create_dir_all(&fakebin).expect("create fake tool directory");
    write_fake_tool(
        &fakebin,
        "agent-spec",
        r#"if [[ "${1:-}" != "plan" ]]; then
  echo "unexpected agent-spec command: ${1:-}" >&2
  exit 64
fi
printf '# fixture smoke plan\n'
"#,
    );
    let path = fake_path(&fakebin);
    let state_dir_arg = state_dir.to_string_lossy().to_string();
    let out = run_script_with_env(
        &[
            "--prepare-only",
            "--run-id",
            "p153-prepare-01",
            "--state-dir",
            &state_dir_arg,
        ],
        &[("PATH", &path)],
    );

    assert!(
        out.status.success(),
        "prepare-only exits 0; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let frozen_spec =
        fs::read_to_string(state_dir.join("frozen.spec.md")).expect("read rendered frozen spec");
    assert!(
        frozen_spec.contains("docs/real-execute-smoke/p153-prepare-01.md"),
        "rendered spec names the run-specific document: {frozen_spec}"
    );
    assert!(
        frozen_spec.contains("crates/agentd-bin/tests/real_execute_smoke_p153_prepare_01.rs"),
        "rendered spec names the run-specific test: {frozen_spec}"
    );
    assert!(
        frozen_spec.contains("AGENTD_REAL_EXECUTE_SMOKE_READY:p153-prepare-01"),
        "rendered spec names the run-specific marker: {frozen_spec}"
    );
    assert!(
        !frozen_spec.contains("template-only"),
        "rendered spec must return to lifecycle verification: {frozen_spec}"
    );
    assert!(state_dir.join("plan.md").is_file(), "plan is run-local");
    assert_eq!(
        fs::read_to_string(state_dir.join("plan.md")).expect("read fake-generated plan"),
        "# fixture smoke plan\n",
        "prepare-only must use the isolated fake agent-spec planner"
    );
    assert!(
        state_dir.join("workflows/execute.dot").is_file(),
        "workflow is run-local"
    );
    let workflow =
        fs::read_to_string(state_dir.join("workflows/execute.dot")).expect("read smoke workflow");
    assert!(workflow.contains("verify_task_delta"), "{workflow}");
    assert!(
        workflow.contains(&state_dir.join("frozen.spec.md").display().to_string()),
        "workflow reads the run-local spec: {workflow}"
    );
    assert!(
        workflow.contains(&state_dir.join("plan.md").display().to_string()),
        "workflow writes the run-local plan: {workflow}"
    );
    assert!(
        workflow.contains(&state_dir.join("report.md").display().to_string()),
        "workflow reads the run-local report: {workflow}"
    );
    let task_base = Command::new("git")
        .args(["-C", repo_root().to_str().unwrap(), "rev-parse", "HEAD"])
        .output()
        .expect("read exact task base");
    assert!(task_base.status.success(), "git rev-parse HEAD succeeds");
    let task_base = String::from_utf8(task_base.stdout)
        .expect("utf8 task base")
        .trim()
        .to_owned();
    let publish_contract = format!(
        "agentd_publish_worktree.sh ${{worktree}} ${{task_run_id}} {task_base} {}",
        state_dir.join("report.md").display()
    );
    assert!(
        workflow.contains(&publish_contract),
        "workflow passes exact base and run-local report to publication: {workflow}"
    );
    assert!(
        !workflow.contains(".agentd/run/"),
        "workflow must not use shared runtime state: {workflow}"
    );
}

#[test]
fn real_execute_smoke_rejects_unsafe_run_id_before_state_creation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_dir = temp.path().join("state");
    let state_dir_arg = state_dir.to_string_lossy().to_string();
    let out = run_script(&[
        "--prepare-only",
        "--run-id",
        "unsafe/run id",
        "--state-dir",
        &state_dir_arg,
    ]);

    assert!(!out.status.success(), "unsafe run id should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("invalid run id") && stderr.contains("unsafe/run id"),
        "stderr explains the unsafe run id: {stderr}"
    );
    assert!(
        !state_dir.exists(),
        "unsafe run id must fail before creating state"
    );
}

#[test]
fn real_execute_smoke_rejects_existing_default_target_before_state_creation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("repo");
    let scripts = root.join("scripts");
    let specs = root.join("specs/e2e");
    let workflows = root.join("workflows");
    let existing_doc = root.join("docs/real-execute-smoke/p153-existing-target.md");
    fs::create_dir_all(&scripts).expect("create scripts directory");
    fs::create_dir_all(&specs).expect("create specs directory");
    fs::create_dir_all(&workflows).expect("create workflows directory");
    fs::create_dir_all(existing_doc.parent().unwrap()).expect("create docs directory");
    fs::copy(script_path(), scripts.join("agentd_real_execute_smoke.sh"))
        .expect("copy smoke script");
    fs::copy(
        repo_root().join("scripts/agentd_write_plan.sh"),
        scripts.join("agentd_write_plan.sh"),
    )
    .expect("copy plan helper");
    fs::copy(
        repo_root().join("specs/e2e/real-execute-smoke-template.spec.md"),
        specs.join("real-execute-smoke-template.spec.md"),
    )
    .expect("copy smoke template");
    fs::copy(
        repo_root().join("workflows/execute.dot"),
        workflows.join("execute.dot"),
    )
    .expect("copy execute workflow");
    fs::write(&existing_doc, "already present at task base\n").expect("write existing target");

    let mut git_init = Command::new("git");
    git_init.current_dir(&root).args(["init"]);
    run_ok(&mut git_init, "git init");
    for args in [
        vec!["config", "user.email", "agentd@example.invalid"],
        vec!["config", "user.name", "agentd test"],
        vec!["add", "."],
        vec!["commit", "-m", "seed fixture"],
    ] {
        let mut git = Command::new("git");
        git.current_dir(&root).args(args);
        run_ok(&mut git, "prepare fixture repository");
    }

    let state_dir = root.join("state");
    let script = scripts.join("agentd_real_execute_smoke.sh");
    let out = run_script_in(
        &script,
        &root,
        &[
            "--prepare-only",
            "--run-id",
            "p153-existing-target",
            "--state-dir",
            state_dir.to_str().unwrap(),
        ],
    );

    assert!(!out.status.success(), "existing default target must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("default smoke target already exists")
            && stderr.contains("docs/real-execute-smoke/p153-existing-target.md"),
        "stderr names the conflicting target: {stderr}"
    );
    assert!(
        !state_dir.exists(),
        "existing target must fail before creating state"
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
    for tool in [
        "cargo",
        "tmux",
        "claude",
        "codex",
        "agent-spec",
        "curl",
        "git",
    ] {
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
    write_fake_tool(&fakebin, "codex", "echo codex 1.0\n");
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
fn real_execute_smoke_codex_only_preflight_accepts_fake_codex_without_claude() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fakebin = temp.path().join("bin");
    fs::create_dir(&fakebin).expect("fakebin");
    write_fake_tool(&fakebin, "cargo", "echo cargo 1.85\n");
    write_fake_tool(&fakebin, "tmux", "echo tmux 3.4\n");
    write_fake_tool(&fakebin, "codex", "echo codex 1.0\n");
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
        "gh",
        "if [[ \"${1:-}\" == \"auth\" && \"${2:-}\" == \"status\" ]]; then exit 0; fi\necho gh\n",
    );

    let state_dir = temp.path().join("state");
    let out = Command::new("bash")
        .arg(script_path())
        .args([
            "--preflight-only",
            "--implementer-role",
            "codex-impl",
            "--reviewers",
            "codex-sec,codex-perf,codex-readability",
            "--state-dir",
            state_dir.to_string_lossy().as_ref(),
        ])
        .current_dir(repo_root())
        .env("PATH", fake_path(&fakebin))
        .output()
        .expect("run execute smoke codex-only preflight");

    assert!(
        out.status.success(),
        "codex-only fake prereqs pass; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("preflight ok"), "{stdout}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("claude"),
        "codex-only preflight must not require claude: {stderr}"
    );
    assert!(
        !state_dir.join("daemon.log").exists(),
        "preflight-only should not start the daemon"
    );
}

#[test]
fn real_execute_smoke_mixed_roles_preflight_requires_claude() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fakebin = temp.path().join("bin");
    fs::create_dir(&fakebin).expect("fakebin");
    for tool in ["cargo", "tmux", "codex", "agent-spec", "curl", "git", "gh"] {
        write_fake_tool(&fakebin, tool, "echo ok\n");
    }

    let state_dir = temp.path().join("state");
    let out = Command::new("bash")
        .arg(script_path())
        .args([
            "--preflight-only",
            "--implementer-role",
            "codex-impl",
            "--reviewers",
            "claude-sec,codex-perf",
            "--state-dir",
            state_dir.to_string_lossy().as_ref(),
        ])
        .current_dir(repo_root())
        .env("PATH", fake_path(&fakebin))
        .output()
        .expect("run execute smoke mixed preflight");

    assert!(!out.status.success(), "mixed roles without claude fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("claude"),
        "stderr should name missing claude prerequisite: {stderr}"
    );
}

#[test]
fn real_execute_smoke_preflight_rejects_no_common_history_before_agents() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fakebin = temp.path().join("bin");
    fs::create_dir(&fakebin).expect("fakebin");
    write_fake_tool(&fakebin, "cargo", "echo cargo 1.85\n");
    write_fake_tool(&fakebin, "tmux", "echo tmux 3.4\n");
    write_fake_tool(&fakebin, "codex", "echo codex 1.0\n");
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
fn real_execute_smoke_dry_run_distinguishes_pr_success_from_captured_preflight_failure() {
    let out = run_script(&["--dry-run", "--run-id", "execute-success-criterion"]);

    assert!(
        out.status.success(),
        "dry-run exits 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("open_pr opens a real PR"),
        "success criterion should require real PR creation: {stdout}"
    );
    assert!(
        stdout.contains("captured preflight error from scripts/agentd_open_pr.sh")
            && stdout.contains("failure evidence, not success"),
        "plan should classify captured open_pr preflight errors as failure evidence: {stdout}"
    );
    assert!(
        !stdout.contains("open_pr either opens a PR or"),
        "plan must not treat open_pr preflight failure as a success alternative: {stdout}"
    );
}

#[test]
fn real_execute_smoke_dry_run_prints_codex_runtime_roles() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_dir = temp.path().join("state");
    let state_dir_arg = state_dir.to_string_lossy().to_string();
    let out = run_script(&[
        "--dry-run",
        "--run-id",
        "codex-only-dry-run",
        "--implementer-role",
        "codex-impl",
        "--reviewers",
        "codex-sec,codex-perf,codex-readability",
        "--state-dir",
        &state_dir_arg,
    ]);

    assert!(
        out.status.success(),
        "dry-run exits 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("implementer_role: codex-impl"), "{stdout}");
    assert!(
        stdout.contains("reviewers: codex-sec,codex-perf,codex-readability"),
        "{stdout}"
    );
    assert!(
        stdout.contains("execute.workflow.dot"),
        "dry-run names the smoke-local workflow copy: {stdout}"
    );
    assert!(
        !state_dir.exists(),
        "dry-run should not create the state directory"
    );
}

#[test]
fn real_execute_smoke_runtime_matrix_dry_run_prints_codex_roles() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_dir = temp.path().join("state");
    let state_dir_arg = state_dir.to_string_lossy().to_string();
    let out = run_script_with_env(
        &[
            "--dry-run",
            "--run-id",
            "runtime-matrix-codex-dry-run",
            "--state-dir",
            &state_dir_arg,
        ],
        &[("AGENTD_REAL_EXECUTE_RUNTIMES", "codex,codex,codex,codex")],
    );

    assert!(
        out.status.success(),
        "dry-run exits 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("runtime_matrix: codex,codex,codex,codex"),
        "{stdout}"
    );
    assert!(stdout.contains("implementer_role: codex-impl"), "{stdout}");
    assert!(
        stdout.contains("reviewers: codex-sec,codex-perf,codex-readability"),
        "{stdout}"
    );
    assert!(
        !state_dir.exists(),
        "dry-run should not create the state directory"
    );
}

#[test]
fn real_execute_smoke_codex_only_success_requires_run_unique_branch_and_pr() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_dir = temp.path().join("state");
    let state_dir_arg = state_dir.to_string_lossy().to_string();
    let run_id = "p153-codex-only-01";
    let out = run_script_with_env(
        &[
            "--dry-run",
            "--run-id",
            run_id,
            "--state-dir",
            &state_dir_arg,
        ],
        &[("AGENTD_REAL_EXECUTE_RUNTIMES", "codex,codex,codex,codex")],
    );

    assert!(
        out.status.success(),
        "Codex-only dry-run exits 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for expected in [
        "runtime_matrix: codex,codex,codex,codex",
        "implementer_role: codex-impl",
        "reviewers: codex-sec,codex-perf,codex-readability",
        "docs/real-execute-smoke/p153-codex-only-01.md",
        "crates/agentd-bin/tests/real_execute_smoke_p153_codex_only_01.rs",
        "publish_branch pushes agentd/<task_run_id>",
        "open_pr opens a real PR",
    ] {
        assert!(
            stdout.contains(expected),
            "Codex-only plan should require {expected}: {stdout}"
        );
    }
    assert!(
        !stdout.contains("implementer_role: claude") && !stdout.contains("reviewers: claude"),
        "Codex-only plan must not select a Claude role: {stdout}"
    );
    assert!(
        !state_dir.exists(),
        "Codex-only dry-run should not create state"
    );
}

#[test]
fn real_execute_smoke_runtime_matrix_codex_only_preflight_does_not_require_claude() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fakebin = temp.path().join("bin");
    fs::create_dir(&fakebin).expect("fakebin");
    write_fake_execute_preflight_tools(&fakebin, false);

    let state_dir = temp.path().join("state");
    let mut command = script_command();
    let out = command
        .args([
            "--preflight-only",
            "--state-dir",
            state_dir.to_string_lossy().as_ref(),
        ])
        .env("PATH", fake_path(&fakebin))
        .env("AGENTD_REAL_EXECUTE_RUNTIMES", "codex,codex,codex,codex")
        .output()
        .expect("run execute smoke runtime-matrix codex-only preflight");

    assert!(
        out.status.success(),
        "codex runtime matrix fake prereqs pass; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("preflight ok"), "{stdout}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("claude"),
        "codex runtime matrix preflight must not require claude: {stderr}"
    );
    assert!(
        !state_dir.join("daemon.log").exists(),
        "preflight-only should not start the daemon"
    );
}

#[test]
fn real_execute_smoke_runtime_matrix_mixed_preflight_requires_claude() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dry_state_dir = temp.path().join("dry-state");
    let dry_state_arg = dry_state_dir.to_string_lossy().to_string();
    let matrix = "codex,claude,codex,codex";
    let dry = run_script_with_env(
        &[
            "--dry-run",
            "--run-id",
            "runtime-matrix-mixed-dry-run",
            "--state-dir",
            &dry_state_arg,
        ],
        &[("AGENTD_REAL_EXECUTE_RUNTIMES", matrix)],
    );
    assert!(
        dry.status.success(),
        "mixed matrix dry-run exits 0; stderr: {}",
        String::from_utf8_lossy(&dry.stderr)
    );
    let dry_stdout = String::from_utf8_lossy(&dry.stdout);
    assert!(
        dry_stdout.contains(&format!("runtime_matrix: {matrix}")),
        "{dry_stdout}"
    );
    assert!(
        dry_stdout.contains("reviewers: claude-sec,codex-perf,codex-readability"),
        "{dry_stdout}"
    );
    assert!(
        !dry_stdout.contains("gemini-readability"),
        "runtime matrix must replace the old default reviewer set: {dry_stdout}"
    );

    let fakebin = temp.path().join("bin");
    fs::create_dir(&fakebin).expect("fakebin");
    write_fake_execute_preflight_tools(&fakebin, false);
    let state_dir = temp.path().join("state");
    let mut command = script_command();
    let out = command
        .args([
            "--preflight-only",
            "--state-dir",
            state_dir.to_string_lossy().as_ref(),
        ])
        .env("PATH", fake_path(&fakebin))
        .env("AGENTD_REAL_EXECUTE_RUNTIMES", matrix)
        .output()
        .expect("run execute smoke runtime-matrix mixed preflight");

    assert!(!out.status.success(), "mixed matrix without claude fails");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("claude"),
        "stderr should name missing claude prerequisite: {stderr}"
    );
    assert!(
        !state_dir.join("daemon.log").exists(),
        "preflight-only should not start the daemon"
    );
}

#[test]
fn real_execute_smoke_runtime_matrix_rejects_wrong_arity_or_unknown_runtime() {
    for (matrix, case_name) in [
        ("codex,codex", "too-few-runtimes"),
        ("codex,gemini,codex,codex", "unsupported-gemini-runtime"),
    ] {
        let temp = tempfile::tempdir().expect("tempdir");
        let state_dir = temp.path().join(case_name);
        let state_dir_arg = state_dir.to_string_lossy().to_string();
        let out = run_script_with_env(
            &[
                "--dry-run",
                "--run-id",
                case_name,
                "--state-dir",
                &state_dir_arg,
            ],
            &[("AGENTD_REAL_EXECUTE_RUNTIMES", matrix)],
        );

        assert!(
            !out.status.success(),
            "invalid runtime matrix {matrix} should fail"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("AGENTD_REAL_EXECUTE_RUNTIMES"),
            "stderr should name the invalid env var for {matrix}: {stderr}"
        );
        assert!(
            !state_dir.exists(),
            "invalid dry-run should not create the state directory"
        );
    }
}

#[test]
fn real_execute_smoke_runtime_matrix_conflicts_with_explicit_roles() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_dir = temp.path().join("state");
    let state_dir_arg = state_dir.to_string_lossy().to_string();
    let out = run_script_with_env(
        &[
            "--dry-run",
            "--run-id",
            "runtime-matrix-conflict",
            "--implementer-role",
            "codex-impl",
            "--state-dir",
            &state_dir_arg,
        ],
        &[("AGENTD_REAL_EXECUTE_RUNTIMES", "codex,codex,codex,codex")],
    );

    assert!(
        !out.status.success(),
        "runtime matrix with explicit role flags should fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("AGENTD_REAL_EXECUTE_RUNTIMES")
            && stderr.contains("explicit")
            && stderr.contains("--implementer-role"),
        "stderr should explain the matrix and explicit role conflict: {stderr}"
    );
    assert!(
        !state_dir.exists(),
        "invalid dry-run should not create the state directory"
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
