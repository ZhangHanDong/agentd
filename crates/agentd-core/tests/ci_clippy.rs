use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &str) -> String {
    fs::read_to_string(repo_root().join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

#[test]
fn ci_clippy_known_warning_patterns_are_absent() {
    assert_core_markers_absent();
    assert_tmux_markers_absent();
    assert_surface_markers_absent();
    assert_agentd_bin_markers_absent();
    assert_ci_workflow_markers_absent();
}

#[test]
fn ci_and_local_gates_classify_non_implementation_specs() {
    let ad_e1 = read("specs/e2e/ad-e1-minimum-security-baseline.spec.md");
    let p272 = read("specs/e2e/p272-runtime-compatibility-port.spec.md");
    let template = read("specs/e2e/real-execute-smoke-template.spec.md");
    let verifier = read("scripts/agentd_verify_spec.sh");
    let changed_guard = read("scripts/agentd_guard_changed_contract.sh");
    let local_gate = read("scripts/check.sh");
    let ci_gate = read(".github/workflows/ci.yml");

    for spec in [&ad_e1, &p272] {
        assert!(
            spec.lines()
                .find(|line| line.starts_with("tags:"))
                .is_some_and(|line| line.contains("design-only")),
            "blocked implementation contracts must declare the design-only tag"
        );
    }
    assert!(
        template
            .lines()
            .find(|line| line.starts_with("tags:"))
            .is_some_and(|line| line.contains("template-only")),
        "unrendered contracts must declare the template-only tag"
    );
    for marker in [
        "agent-spec parse \"$spec\" --format json",
        "index(\"design-only\")",
        "index(\"template-only\")",
        "agent-spec lint \"$spec\" --min-score 0.7 --format text",
        "agent-spec lifecycle \"$spec\" --code . --min-score 0.7 --format text",
    ] {
        assert!(
            verifier.contains(marker),
            "spec verifier must contain {marker}"
        );
    }
    for gate in [&local_gate, &ci_gate] {
        assert!(
            gate.contains("scripts/agentd_verify_spec.sh \"$spec\""),
            "every spec gate must use the shared verifier"
        );
        assert!(
            gate.contains("scripts/agentd_guard_changed_contract.sh"),
            "every spec gate must run the changed-contract boundary guard"
        );
    }
    assert!(
        ci_gate.contains("scripts/agentd_guard_changed_contract.sh --range")
            && ci_gate.contains("github.event.pull_request.base.sha")
            && ci_gate.contains("fetch-depth: 0"),
        "PR checks must guard the complete base-to-head range after P156 adoption"
    );
    assert!(
        local_gate.contains("agent-spec 1.0.0")
            && ci_gate.contains("cargo install --locked agent-spec --version 1.0.0")
            && !ci_gate.contains("cargo install --locked agent-spec || true"),
        "local and CI gates must require the same pinned agent-spec version"
    );
    for marker in [
        "git diff --cached",
        "git diff-tree",
        "agent-spec lifecycle",
        "--change",
    ] {
        assert!(
            changed_guard.contains(marker),
            "changed-contract guard must contain {marker}"
        );
    }

    for (path, selector) in [
        (
            "specs/e2e/p100-worktree-pr-publication.spec.md",
            "production_runhost_execute_tools_use_stable_repo_cwd_after_review_fan_in",
        ),
        (
            "specs/e2e/p120-agent-mcp-stdio-startup-context.spec.md",
            "mcp_stdio_command_includes_proxy_url_to_daemon",
        ),
        (
            "specs/e2e/p130-open-pr-preflight.spec.md",
            "production_runhost_execute_tools_use_stable_repo_cwd_after_review_fan_in",
        ),
    ] {
        assert!(
            read(path).contains(selector),
            "implemented contract {path} must use current lifecycle selector {selector}"
        );
    }
}

fn assert_core_markers_absent() {
    let codergen = read("crates/agentd-core/src/handler/codergen.rs");
    assert!(
        !codergen.contains(".map(|path| path.display().to_string())\n        .unwrap_or_else(|_| \"<unknown>\".to_string())"),
        "codergen current_dir fallback should use map_or_else"
    );

    let fan_in = read("crates/agentd-core/src/handler/fan_in.rs");
    assert!(
        !fan_in.contains("levenshtein_chars(&previous, &current) as f64 / max_len as f64"),
        "normalized text diff should avoid direct usize-to-f64 casts"
    );

    let fan_out = read("crates/agentd-core/src/handler/fan_out.rs");
    assert!(
        !fan_out.contains(".map(Path::new)\n        .unwrap_or_else(|| Path::new(\".\"))"),
        "review worktree fallback should use map_or_else"
    );
    assert!(
        !fan_out.contains(".map(|path| path.display().to_string())\n        .unwrap_or_else(|_| \"<unknown>\".to_string())"),
        "fan_out current_dir fallback should use map_or_else"
    );
    assert!(
        !fan_out.contains("fn review_prompt(\n    ctx: &HandlerCtx<'_>,"),
        "review_prompt should not keep the 9-argument signature"
    );

    let allocator = read("crates/agentd-core/src/ports/worktree_allocator.rs");
    assert!(
        !allocator.contains("keyed by the task_run id"),
        "task_run should be backticked in worktree allocator docs"
    );

    let in_memory_store = read("crates/agentd-core/src/test_support/in_memory_store.rs");
    assert!(
        !in_memory_store.contains("read of a task_run's"),
        "task_run should be backticked in in-memory store docs"
    );

    let tmux_pool = read("crates/agentd-tmux/src/pool.rs");
    assert!(
        !tmux_pool.contains(".filter_map(|path| path.file_name().map(|name| name.to_os_string()))"),
        "pool preserve-name collection should avoid redundant closure"
    );
    assert!(
        !tmux_pool.contains("let dst = dest.join(&name);"),
        "sync_dir_contents should avoid dest/dst similar names"
    );
    assert!(
        !tmux_pool.contains("panic!(\"git {}: {err}\", args.join(\" \"))"),
        "tmux pool test git helpers should avoid clippy::panic"
    );

    let handlers_park = read("crates/agentd-core/tests/handlers_park.rs");
    assert!(
        !handlers_park.contains("req.worktree == PathBuf::from(\"/tmp/agentd-task-wt\")"),
        "handler tests should avoid owned PathBuf comparisons"
    );
    assert!(
        !handlers_park.contains("req.worktree != PathBuf::from(\"/tmp/agentd-task-wt\")"),
        "handler tests should avoid owned PathBuf comparisons"
    );
}

fn assert_tmux_markers_absent() {
    let tmux_pool = read("crates/agentd-tmux/src/pool.rs");
    assert!(
        !tmux_pool.contains(".filter_map(|path| path.file_name().map(|name| name.to_os_string()))"),
        "pool preserve-name collection should avoid redundant closure"
    );
    assert!(
        !tmux_pool.contains("let dst = dest.join(&name);"),
        "sync_dir_contents should avoid dest/dst similar names"
    );
    assert!(
        !tmux_pool.contains("panic!(\"git {}: {err}\", args.join(\" \"))"),
        "tmux pool test git helpers should avoid clippy::panic"
    );

    let tmux_pool_test = read("crates/agentd-tmux/tests/pool.rs");
    assert!(
        !tmux_pool_test.contains("create fake worktree {p:?}: {e}"),
        "tmux pool tests should avoid unnecessary debug formatting"
    );
}

fn assert_surface_markers_absent() {
    let surface_http = read("crates/agentd-surface/tests/http.rs");
    assert!(
        !surface_http.contains("r#\"fetch(`/runs/${\"#"),
        "surface HTTP tests should avoid needless raw string hashes"
    );
    assert!(
        !surface_http.contains("r#\"new EventSource(`/runs/${\"#"),
        "surface HTTP tests should avoid needless raw string hashes"
    );
    assert!(
        !surface_http.contains("r#\"/events\"#"),
        "surface HTTP tests should avoid needless raw string hashes"
    );
}

fn assert_agentd_bin_markers_absent() {
    let agentd_cli = read("crates/agentd-bin/src/cli.rs");
    assert!(
        !agentd_cli.contains("panic!(\"expected cleanup-worktrees command\")"),
        "CLI tests should avoid explicit panic markers denied by clippy"
    );

    let stdio_mcp = read("crates/agentd-bin/src/stdio_mcp.rs");
    assert!(
        !stdio_mcp
            .contains("fn success_response(id: Value, result: Value) -> Value {\n    json!({"),
        "stdio success_response should consume owned Values directly"
    );

    let agentd_main = read("crates/agentd-bin/src/main.rs");
    assert!(
        !agentd_main.contains(".await.map(|()| ())"),
        "agentd main should avoid identity result maps"
    );

    let agentd_contract = read("crates/agentd-bin/tests/contract.rs");
    assert!(
        !agentd_contract.contains("async fn paths(&self) -> Vec<PathBuf>"),
        "contract test helpers should not be async without await"
    );
}

fn assert_ci_workflow_markers_absent() {
    let ci_workflow = read(".github/workflows/ci.yml");
    assert!(
        !ci_workflow.contains("arguments: --all-features check"),
        "cargo-deny action should not receive a duplicate check subcommand"
    );

    let scaffold = read("crates/agentd-core/tests/scaffold.rs");
    assert!(
        !scaffold.contains("Command::new(\"rg\")"),
        "scaffold boundary tests should not require ripgrep on CI runners"
    );
}
