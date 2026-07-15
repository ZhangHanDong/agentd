//! Task 1 skeleton: tmux discovery, `BackendError` → `CoreError` mapping,
//! `Config`, the production `TokioCommandRunner`, and the `TmuxBackend` `tmux`
//! helper. Test names match the `specs/tmux/p1-discovery-and-skeleton.spec.md`
//! selectors.
//!
//! Integration tests are a separate crate, so they do not inherit the lib's
//! `#![warn(clippy::unwrap_used, clippy::panic)]` opt-ins.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agentd_core::CoreError;
use agentd_core::ports::{CommandRunner, RunOpts};
use agentd_core::test_support::RecordingCommandRunner;
use agentd_core::types::CliKind;

use agentd_tmux::discovery::resolve_tmux_bin;
use agentd_tmux::{BackendError, Config, TmuxBackend, TokioCommandRunner};

// ---- discovery (§4.4) -----------------------------------------------------

#[test]
fn discovery_honors_env_override() {
    // The override wins even though the existence predicate says nothing exists.
    let got = resolve_tmux_bin(
        Some(PathBuf::from("/custom/tmux")),
        &[PathBuf::from("/opt/homebrew/bin/tmux")],
        |_| false,
        || None,
    )
    .expect("env override returns Ok");
    assert_eq!(got, PathBuf::from("/custom/tmux"));
}

#[test]
fn discovery_selects_first_existing_candidate() {
    let candidates = [PathBuf::from("/a/tmux"), PathBuf::from("/b/tmux")];
    let got = resolve_tmux_bin(None, &candidates, |p| p == Path::new("/b/tmux"), || None)
        .expect("an existing candidate is selected");
    assert_eq!(got, PathBuf::from("/b/tmux"));
}

#[test]
fn discovery_missing_tmux_is_fatal_with_hint() {
    let err = resolve_tmux_bin(None, &[PathBuf::from("/a/tmux")], |_| false, || None)
        .expect_err("nothing found is Fatal");
    match err {
        BackendError::Fatal(msg) => {
            assert!(msg.contains("tmux not found"), "msg: {msg}");
            assert!(msg.contains("AGENTD_TMUX_BIN"), "msg: {msg}");
        }
        other => panic!("expected Fatal, got {other:?}"),
    }
}

// ---- error mapping (§4.2, D2) ---------------------------------------------

#[test]
fn backend_error_recoverable_maps_to_core_backend() {
    let core: CoreError = BackendError::Recoverable("rebind instead".into()).into();
    match core {
        CoreError::Backend(s) => assert!(s.contains("rebind instead"), "s: {s}"),
        other => panic!("expected CoreError::Backend, got {other:?}"),
    }
}

#[test]
fn backend_error_fatal_maps_to_core_backend() {
    let core: CoreError = BackendError::Fatal("tmux not found".into()).into();
    match core {
        CoreError::Backend(s) => assert!(s.contains("tmux not found"), "s: {s}"),
        other => panic!("expected CoreError::Backend, got {other:?}"),
    }
}

// ---- config (§4.6/§4.7) ---------------------------------------------------

#[test]
fn config_ready_patterns_default_and_override() {
    let mut cfg = Config::default();
    assert!(
        !cfg.ready_patterns.claude_code.is_empty(),
        "default claude_code ready patterns should be non-empty"
    );

    cfg.ready_patterns.claude_code = vec!["MYPROMPT>".to_string()];
    assert!(
        cfg.main_prompt_visible("noise\nMYPROMPT> ready\n", CliKind::ClaudeCode),
        "overridden pattern should be recognized in the buffer"
    );
    assert!(
        !cfg.main_prompt_visible("nothing relevant here", CliKind::ClaudeCode),
        "a buffer without the pattern is not the main prompt"
    );
}

#[test]
fn config_default_ready_deadline_covers_slow_codex_mcp_startup() {
    let cfg = Config::default();
    assert!(
        cfg.ready_deadline >= Duration::from_secs(45),
        "default ready deadline should cover 30s Codex MCP startup timeouts, got {:?}",
        cfg.ready_deadline
    );
}

#[test]
fn config_codex_ready_patterns_do_not_match_banner_only() {
    let cfg = Config::default();
    assert!(
        !cfg.main_prompt_visible(
            "╭──────────────────────────────────────────────────────╮\n\
             │ >_ OpenAI Codex (v0.143.0)                           │\n\
             ╰──────────────────────────────────────────────────────╯\n",
            CliKind::Codex
        ),
        "Codex banner alone is not enough; wait for the idle prompt marker"
    );
}

// ---- TokioCommandRunner (design D6) ---------------------------------------

#[tokio::test]
async fn tokio_runner_captures_stdout() {
    let runner = TokioCommandRunner::new();
    let out = runner
        .run("printf", &["hello-stdout".to_string()], RunOpts::default())
        .await
        .expect("printf runs to completion");
    assert_eq!(out.status, 0);
    assert!(
        out.stdout.contains("hello-stdout"),
        "stdout: {:?}",
        out.stdout
    );
}

#[tokio::test]
async fn tokio_runner_nonzero_exit_is_ok() {
    let runner = TokioCommandRunner::new();
    let out = runner
        .run("false", &[], RunOpts::default())
        .await
        .expect("`false` runs to completion (non-zero exit is Ok, not Err)");
    assert_ne!(out.status, 0);
}

#[tokio::test]
async fn tokio_runner_launch_failure_is_error() {
    let runner = TokioCommandRunner::new();
    let res = runner
        .run(
            "definitely-not-a-real-binary-xyzzy-12345",
            &[],
            RunOpts::default(),
        )
        .await;
    assert!(res.is_err(), "a missing program must be Err, got {res:?}");
}

#[tokio::test]
async fn tokio_runner_forwards_stdin() {
    let runner = TokioCommandRunner::new();
    let opts = RunOpts {
        stdin: Some(b"piped-payload".to_vec()),
        ..RunOpts::default()
    };
    let out = runner
        .run("cat", &[], opts)
        .await
        .expect("cat runs to completion");
    assert!(
        out.stdout.contains("piped-payload"),
        "stdout: {:?}",
        out.stdout
    );
}

// ---- TmuxBackend tmux() helper (D3) ---------------------------------------

#[tokio::test]
async fn tmux_helper_runs_resolved_binary_and_records_argv() {
    let rec = Arc::new(RecordingCommandRunner::new());
    let backend = TmuxBackend::new(
        rec.clone(),
        PathBuf::from("/opt/homebrew/bin/tmux"),
        Config::default(),
    );

    backend
        .tmux(&["has-session", "-t", "agentd-x"], RunOpts::default())
        .await
        .expect("tmux helper runs through the injected runner");

    let calls = rec.calls();
    assert_eq!(calls.len(), 1, "exactly one recorded call");
    assert_eq!(calls[0].program, "/opt/homebrew/bin/tmux");
    assert_eq!(
        calls[0].args,
        vec![
            "has-session".to_string(),
            "-t".to_string(),
            "agentd-x".to_string()
        ]
    );
}
