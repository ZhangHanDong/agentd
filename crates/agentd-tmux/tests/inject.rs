//! Task 3: prompt injection (buffer path, §4.6) + `wait_for_ready` (§4.7), and
//! their wiring into spawn step 5. Test names match
//! `specs/tmux/p3-prompt-injection.spec.md`. All timing comes from Config, set
//! to zero here so the tests do not actually sleep.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use agentd_core::ports::{AgentBackend, CommandError, CommandOutput};
use agentd_core::test_support::RecordingCommandRunner;
use agentd_core::types::{
    AgentHandle, AgentId, BackendKind, CliKind, LaunchStrategy, SpawnRequest,
};

use agentd_tmux::{BackendError, Config, TmuxBackend};

const TMUX_BIN: &str = "/opt/homebrew/bin/tmux";

#[allow(clippy::unnecessary_wraps)]
fn ok(stdout: &str, status: i32) -> Result<CommandOutput, CommandError> {
    Ok(CommandOutput {
        stdout: stdout.to_string(),
        stderr: String::new(),
        status,
    })
}

fn zero_delay_cfg() -> Config {
    Config {
        inject_delay: Duration::ZERO,
        ready_probe_initial: Duration::ZERO,
        ready_probe_max: Duration::ZERO,
        ..Config::default()
    }
}

fn backend(rec: &Arc<RecordingCommandRunner>, cfg: Config) -> TmuxBackend {
    TmuxBackend::new(rec.clone(), TMUX_BIN.into(), cfg)
}

fn handle(address: &str) -> AgentHandle {
    AgentHandle {
        agent_id: AgentId::parsed("claude-impl-a"),
        backend: BackendKind::Tmux,
        address: address.to_string(),
        pane_id: Some("%1".to_string()),
        pid: Some(1),
        session_name: "agentd-claude-impl-a".to_string(),
        spawned_at: SystemTime::UNIX_EPOCH,
    }
}

fn first_arg(call: &agentd_core::test_support::RecordedCall) -> Option<&str> {
    call.args.first().map(String::as_str)
}

#[tokio::test]
async fn send_prompt_uses_buffer_path() {
    let rec = Arc::new(RecordingCommandRunner::new());
    backend(&rec, zero_delay_cfg())
        .send_prompt(&handle("agentd-x:0.0"), "hello world")
        .await
        .expect("send_prompt ok");

    let calls = rec.calls();
    assert_eq!(
        calls[0].args,
        vec!["set-buffer".to_string(), "hello world".to_string()],
        "stage 1 loads the buffer by argv"
    );
    let paste = calls
        .iter()
        .find(|c| first_arg(c) == Some("paste-buffer"))
        .expect("a paste-buffer call");
    assert!(
        paste.args.contains(&"-t".to_string()) && paste.args.contains(&"agentd-x:0.0".to_string()),
        "paste targets the pane: {:?}",
        paste.args
    );
    assert!(
        paste.args.contains(&"-p".to_string()),
        "bracketed paste (-p): {:?}",
        paste.args
    );
    assert!(
        paste.args.contains(&"-d".to_string()),
        "delete the buffer after pasting (-d): {:?}",
        paste.args
    );
    let last = calls.last().expect("at least one call");
    assert_eq!(
        last.args,
        vec![
            "send-keys".to_string(),
            "-t".to_string(),
            "agentd-x:0.0".to_string(),
            "Enter".to_string()
        ],
        "stage 4 is a single bare Enter"
    );
}

#[tokio::test]
async fn send_prompt_never_sends_payload_as_keys() {
    let rec = Arc::new(RecordingCommandRunner::new());
    backend(&rec, zero_delay_cfg())
        .send_prompt(&handle("agentd-x:0.0"), "secret-payload")
        .await
        .expect("send_prompt ok");

    for call in rec.calls() {
        if first_arg(&call) == Some("send-keys") {
            assert!(
                !call.args.iter().any(|a| a.contains("secret-payload")),
                "payload must never be a send-keys arg: {:?}",
                call.args
            );
            assert!(
                !call.args.iter().any(|a| a == "-l"),
                "send-keys must never use the -l flag: {:?}",
                call.args
            );
        }
    }
}

#[tokio::test]
async fn send_prompt_large_prompt_uses_stdin() {
    let rec = Arc::new(RecordingCommandRunner::new());
    let big = "x".repeat(64 * 1024 + 1);
    backend(&rec, zero_delay_cfg())
        .send_prompt(&handle("agentd-x:0.0"), &big)
        .await
        .expect("send_prompt ok");

    let calls = rec.calls();
    assert_eq!(
        calls[0].args,
        vec!["set-buffer".to_string(), "-".to_string()],
        "large prompt loads via the stdin marker, not argv"
    );
    for call in &calls {
        assert!(
            !call.args.iter().any(|a| a.contains(big.as_str())),
            "the payload must not appear in any argv (it went via stdin)"
        );
    }
}

#[tokio::test]
async fn wait_for_ready_returns_ok_when_visible() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("loading...\n? for shortcuts\n", 0)); // default claude_code pattern
    backend(&rec, zero_delay_cfg())
        .wait_for_ready(&handle("agentd-x:0.0"), CliKind::ClaudeCode)
        .await
        .expect("ready");

    let calls = rec.calls();
    assert_eq!(calls.len(), 1, "matched on the first capture");
    assert_eq!(first_arg(&calls[0]), Some("capture-pane"));
}

#[tokio::test]
async fn wait_for_ready_loops_until_visible() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("still booting\n", 0)); // capture 1: no ready pattern
    rec.push_output(ok("ready ? for shortcuts\n", 0)); // capture 2: matches
    backend(&rec, zero_delay_cfg())
        .wait_for_ready(&handle("agentd-x:0.0"), CliKind::ClaudeCode)
        .await
        .expect("ready on the second poll");
    assert_eq!(rec.calls().len(), 2, "re-polled until the prompt appeared");
}

#[tokio::test]
async fn wait_for_ready_times_out() {
    let rec = Arc::new(RecordingCommandRunner::new());
    let cfg = Config {
        ready_deadline: Duration::ZERO,
        ..zero_delay_cfg()
    };
    let err = backend(&rec, cfg)
        .wait_for_ready(&handle("agentd-x:0.0"), CliKind::ClaudeCode)
        .await
        .expect_err("never becomes ready");
    match err {
        BackendError::Recoverable(s) => {
            assert!(s.contains("main prompt"), "message: {s}");
        }
        other => panic!("expected Recoverable, got {other:?}"),
    }
}

#[tokio::test]
async fn spawn_injects_initial_prompt_after_ready() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("", 1)); // has-session: none
    rec.push_output(ok("", 0)); // new-session
    rec.push_output(ok("%1 5\n", 0)); // display-message
    rec.push_output(ok("? for shortcuts\n", 0)); // wait_for_ready capture → ready
    rec.push_output(ok("", 0)); // set-buffer
    rec.push_output(ok("", 0)); // paste-buffer
    rec.push_output(ok("", 0)); // send-keys Enter

    let dir = tempfile::tempdir().expect("tempdir");
    let request = SpawnRequest {
        agent_id: AgentId::parsed("claude-impl-a"),
        mxid: None,
        cli: CliKind::ClaudeCode,
        worktree: dir.path().to_path_buf(),
        initial_prompt: Some("go".to_string()),
        env_overrides: HashMap::new(),
        launch_strategy: LaunchStrategy::Direct,
    };
    backend(&rec, zero_delay_cfg())
        .spawn(request)
        .await
        .expect("spawn + inject ok");

    let calls = rec.calls();
    let subs: Vec<&str> = calls.iter().map(|c| c.args[0].as_str()).collect();
    assert_eq!(
        subs,
        [
            "has-session",
            "new-session",
            "display-message",
            "capture-pane",
            "set-buffer",
            "paste-buffer",
            "send-keys",
        ],
        "the wired flow probes, waits for ready, then injects via the buffer path"
    );
}
