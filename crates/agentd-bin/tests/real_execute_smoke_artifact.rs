const SMOKE_ARTIFACT: &str = include_str!("../../../docs/agentd-real-execute-smoke.md");
const READY_MARKER: &str = "AGENTD_REAL_EXECUTE_SMOKE_READY";

#[test]
fn real_execute_smoke_artifact_exists() {
    assert!(
        !SMOKE_ARTIFACT.trim().is_empty(),
        "real execute smoke artifact document should exist and not be empty"
    );
}

#[test]
fn real_execute_smoke_artifact_mentions_ready_marker() {
    assert!(
        SMOKE_ARTIFACT.contains(READY_MARKER),
        "real execute smoke artifact document should contain {READY_MARKER}"
    );
}
