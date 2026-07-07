const SMOKE_DOC: &str = include_str!("../../../docs/agentd-real-execute-smoke.md");

#[test]
fn real_execute_smoke_artifact_exists() {
    assert!(
        !SMOKE_DOC.trim().is_empty(),
        "real execute smoke document should not be empty"
    );
}

#[test]
fn real_execute_smoke_artifact_mentions_ready_marker() {
    assert!(
        SMOKE_DOC.contains("AGENTD_REAL_EXECUTE_SMOKE_READY"),
        "real execute smoke document should contain the ready marker"
    );
}
