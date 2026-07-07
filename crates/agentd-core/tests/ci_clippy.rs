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

    let tmux_pool_test = read("crates/agentd-tmux/tests/pool.rs");
    assert!(
        !tmux_pool_test.contains("create fake worktree {p:?}: {e}"),
        "tmux pool tests should avoid unnecessary debug formatting"
    );

    let ci_workflow = read(".github/workflows/ci.yml");
    assert!(
        !ci_workflow.contains("arguments: --all-features check"),
        "cargo-deny action should not receive a duplicate check subcommand"
    );
}
