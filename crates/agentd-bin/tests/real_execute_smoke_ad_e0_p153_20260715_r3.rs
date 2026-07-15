const ARTIFACT: &str = include_str!("../../../docs/real-execute-smoke/ad-e0-p153-20260715-r3.md");

#[test]
fn real_execute_smoke_ad_e0_p153_20260715_r3_artifact_exists() {
    assert!(!ARTIFACT.is_empty());
}

#[test]
fn real_execute_smoke_ad_e0_p153_20260715_r3_artifact_mentions_ready_marker() {
    assert!(ARTIFACT.contains("AGENTD_REAL_EXECUTE_SMOKE_READY:ad-e0-p153-20260715-r3"));
}
