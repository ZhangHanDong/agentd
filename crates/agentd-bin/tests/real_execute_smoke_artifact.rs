const SMOKE_ARTIFACT: &str = include_str!("../../../docs/agentd-real-execute-smoke.md");

#[test]
fn real_execute_smoke_artifact_exists() {
    assert!(!SMOKE_ARTIFACT.trim().is_empty());
}

#[test]
fn real_execute_smoke_artifact_mentions_ready_marker() {
    assert!(SMOKE_ARTIFACT.contains("AGENTD_REAL_EXECUTE_SMOKE_READY"));
}
