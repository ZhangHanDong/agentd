//! Tasks 4–5: pane capture + status detection (§4.8), and shutdown + rebind
//! (§4.9/§4.10). Test names match `specs/tmux/p4-capture-status.spec.md` and
//! `specs/tmux/p5-shutdown-rebind.spec.md`. Timing comes from Config, zeroed.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use agentd_core::ports::{CommandError, CommandOutput};
use agentd_core::test_support::RecordingCommandRunner;
use agentd_core::types::{AgentHandle, AgentId, AgentStatus, BackendKind};

use agentd_tmux::{BackendError, CaptureOpts, Config, TmuxBackend};

const TMUX_BIN: &str = "/opt/homebrew/bin/tmux";

#[allow(clippy::unnecessary_wraps)]
fn ok(stdout: &str, status: i32) -> Result<CommandOutput, CommandError> {
    Ok(CommandOutput {
        stdout: stdout.to_string(),
        stderr: String::new(),
        status,
    })
}

fn err() -> Result<CommandOutput, CommandError> {
    Err(CommandError {
        message: "boom".to_string(),
        stderr: String::new(),
        status: None,
    })
}

fn zero_gap_cfg() -> Config {
    Config {
        status_diff_gap: Duration::ZERO,
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

// ---- capture (§4.8) -------------------------------------------------------

#[tokio::test]
async fn capture_returns_pane_buffer() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("screen contents", 0));
    let out = backend(&rec, Config::default())
        .capture(
            &handle("agentd-x:0.0"),
            CaptureOpts {
                lines: 200,
                ansi: false,
            },
        )
        .await
        .expect("capture ok");
    assert_eq!(out, "screen contents");

    let call = &rec.calls()[0];
    assert_eq!(call.args[0], "capture-pane");
    assert!(
        call.args.contains(&"-S".to_string()) && call.args.contains(&"-200".to_string()),
        "captures scrollback: {:?}",
        call.args
    );
    assert!(!call.args.contains(&"-e".to_string()), "no ansi requested");
}

#[tokio::test]
async fn capture_with_ansi_includes_escapes() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("x", 0));
    backend(&rec, Config::default())
        .capture(
            &handle("agentd-x:0.0"),
            CaptureOpts {
                lines: 50,
                ansi: true,
            },
        )
        .await
        .expect("capture ok");
    assert!(
        rec.calls()[0].args.contains(&"-e".to_string()),
        "ansi adds -e: {:?}",
        rec.calls()[0].args
    );
}

#[tokio::test]
async fn capture_surfaces_runner_error() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(err());
    let result = backend(&rec, Config::default())
        .capture(
            &handle("agentd-x:0.0"),
            CaptureOpts {
                lines: 50,
                ansi: false,
            },
        )
        .await;
    assert!(matches!(result, Err(BackendError::Recoverable(_))));
}

// ---- status (§4.8) --------------------------------------------------------

#[tokio::test]
async fn status_gone_when_pane_absent() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("", 0)); // empty pane_current_command
    let status = backend(&rec, zero_gap_cfg())
        .status(&handle("agentd-x:0.0"))
        .await
        .expect("status ok");
    assert_eq!(status, AgentStatus::Gone);
}

#[tokio::test]
async fn status_starting_for_booting_shell() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("bash\n", 0)); // pane is a shell
    rec.push_output(ok("\n", 0)); // capture shows nothing yet
    let status = backend(&rec, zero_gap_cfg())
        .status(&handle("agentd-x:0.0"))
        .await
        .expect("status ok");
    assert_eq!(status, AgentStatus::Starting);
}

#[tokio::test]
async fn status_idle_when_output_unchanged() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("claude\n", 0)); // a CLI is running
    rec.push_output(ok("frame-A", 0));
    rec.push_output(ok("frame-A", 0)); // identical → quiescent
    let status = backend(&rec, zero_gap_cfg())
        .status(&handle("agentd-x:0.0"))
        .await
        .expect("status ok");
    assert!(matches!(status, AgentStatus::Idle { .. }), "got {status:?}");
}

#[tokio::test]
async fn status_busy_when_output_changes() {
    let rec = Arc::new(RecordingCommandRunner::new());
    rec.push_output(ok("claude\n", 0));
    rec.push_output(ok("frame-A", 0));
    rec.push_output(ok("frame-B", 0)); // changed → busy
    let status = backend(&rec, zero_gap_cfg())
        .status(&handle("agentd-x:0.0"))
        .await
        .expect("status ok");
    assert!(matches!(status, AgentStatus::Busy { .. }), "got {status:?}");
}
