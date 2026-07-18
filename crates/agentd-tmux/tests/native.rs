use std::time::Duration;

use agentd_tmux::native::{
    NativeProcessConfig, NativeProcessEvent, NativeProcessStatus, NativeRuntime,
};

#[test]
fn native_runtime_executes_through_a_pty_and_bounds_output() {
    let runtime = NativeRuntime::spawn(NativeProcessConfig {
        program: "sh".into(),
        args: vec!["-c".into(), "printf 'ready\\n'; exit 7".into()],
        output_capacity: 8,
        ..NativeProcessConfig::default()
    })
    .expect("native pty spawn");

    let event = runtime.wait(Duration::from_secs(5)).expect("wait");
    assert!(matches!(
        event,
        NativeProcessEvent::Exited { code: Some(7), .. }
    ));
    assert_eq!(
        runtime.status(),
        NativeProcessStatus::Exited { code: Some(7) }
    );
    assert!(runtime.output().len() <= 8);
}

#[test]
fn native_runtime_reports_missing_process_as_gone() {
    let runtime = NativeRuntime::spawn(NativeProcessConfig {
        program: "sh".into(),
        args: vec!["-c".into(), "exit 0".into()],
        ..NativeProcessConfig::default()
    })
    .expect("native pty spawn");

    let _ = runtime.wait(Duration::from_secs(5)).expect("wait");
    assert!(runtime.is_terminal());
    assert!(runtime.native_session_ref().is_none());
}

#[test]
fn native_runtime_writes_input_to_the_pty() {
    let runtime = NativeRuntime::spawn(NativeProcessConfig {
        program: "sh".into(),
        args: vec![
            "-c".into(),
            "read value; printf 'echo:%s\\n' \"$value\"".into(),
        ],
        ..NativeProcessConfig::default()
    })
    .expect("native pty spawn");

    runtime.write(b"hello\n").expect("write to pty");
    let event = runtime.wait(Duration::from_secs(5)).expect("wait");
    assert!(matches!(
        event,
        NativeProcessEvent::Exited { code: Some(0), .. }
    ));
    assert!(
        runtime
            .output()
            .windows(b"echo:hello".len())
            .any(|window| { window == b"echo:hello" })
    );
}
