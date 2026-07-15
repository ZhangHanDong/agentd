use agentd_core::types::{AuditEventId, ExecutionArtifactId};

fn accepts_artifact(_: &ExecutionArtifactId) {}

fn accepts_event(_: &AuditEventId) {}

#[test]
fn enterprise_artifact_and_audit_ids_are_distinct() {
    let artifact = ExecutionArtifactId::new();
    let event = AuditEventId::new();

    assert!(artifact.as_str().starts_with("ar_"));
    assert!(event.as_str().starts_with("ae_"));
    assert_eq!(artifact.as_str().len(), 29);
    assert_eq!(event.as_str().len(), 29);
    assert_ne!(artifact.as_str(), event.as_str());

    accepts_artifact(&artifact);
    accepts_event(&event);
}
