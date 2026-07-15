use agentd_core::types::{
    AgentProfileId, AgentProfileStatus, RuntimeAttemptId, RuntimeAttemptStatus, RuntimeSessionId,
    RuntimeSessionStatus, WorkerId, WorkerIncarnationId, WorkerStatus,
};

#[test]
fn enterprise_identity_ids_and_states_are_distinct_and_round_trip() {
    let profile = AgentProfileId::new();
    let worker = WorkerId::new();
    let incarnation = WorkerIncarnationId::new();
    let session = RuntimeSessionId::new();
    let attempt = RuntimeAttemptId::new();

    assert!(profile.as_str().starts_with("ap_"));
    assert!(worker.as_str().starts_with("wk_"));
    assert!(incarnation.as_str().starts_with("wi_"));
    assert!(session.as_str().starts_with("rs_"));
    assert!(attempt.as_str().starts_with("ra_"));
    assert_eq!(profile.as_str().len(), 29);
    assert_eq!(worker.as_str().len(), 29);
    assert_eq!(incarnation.as_str().len(), 29);
    assert_eq!(session.as_str().len(), 29);
    assert_eq!(attempt.as_str().len(), 29);

    for value in [
        AgentProfileStatus::Active,
        AgentProfileStatus::Disabled,
        AgentProfileStatus::Retired,
    ] {
        assert_eq!(AgentProfileStatus::try_from(value.as_str()), Ok(value));
    }
    assert!(!AgentProfileStatus::Active.is_terminal());
    assert!(!AgentProfileStatus::Disabled.is_terminal());
    assert!(AgentProfileStatus::Retired.is_terminal());

    for value in [
        WorkerStatus::Online,
        WorkerStatus::Draining,
        WorkerStatus::Offline,
        WorkerStatus::Retired,
    ] {
        assert_eq!(WorkerStatus::try_from(value.as_str()), Ok(value));
    }
    assert!(!WorkerStatus::Online.is_terminal());
    assert!(!WorkerStatus::Draining.is_terminal());
    assert!(!WorkerStatus::Offline.is_terminal());
    assert!(WorkerStatus::Retired.is_terminal());

    for value in [
        RuntimeSessionStatus::Requested,
        RuntimeSessionStatus::Starting,
        RuntimeSessionStatus::Running,
        RuntimeSessionStatus::ResumePending,
        RuntimeSessionStatus::Completed,
        RuntimeSessionStatus::Failed,
        RuntimeSessionStatus::Cancelled,
        RuntimeSessionStatus::Lost,
    ] {
        assert_eq!(RuntimeSessionStatus::try_from(value.as_str()), Ok(value));
    }
    for value in [
        RuntimeSessionStatus::Requested,
        RuntimeSessionStatus::Starting,
        RuntimeSessionStatus::Running,
        RuntimeSessionStatus::ResumePending,
    ] {
        assert!(!value.is_terminal());
    }
    for value in [
        RuntimeSessionStatus::Completed,
        RuntimeSessionStatus::Failed,
        RuntimeSessionStatus::Cancelled,
        RuntimeSessionStatus::Lost,
    ] {
        assert!(value.is_terminal());
    }

    for value in [
        RuntimeAttemptStatus::Starting,
        RuntimeAttemptStatus::Running,
        RuntimeAttemptStatus::Exited,
        RuntimeAttemptStatus::Gone,
    ] {
        assert_eq!(RuntimeAttemptStatus::try_from(value.as_str()), Ok(value));
    }
    assert!(!RuntimeAttemptStatus::Starting.is_terminal());
    assert!(!RuntimeAttemptStatus::Running.is_terminal());
    assert!(RuntimeAttemptStatus::Exited.is_terminal());
    assert!(RuntimeAttemptStatus::Gone.is_terminal());

    assert!(AgentProfileStatus::try_from("online").is_err());
    assert!(WorkerStatus::try_from("active").is_err());
    assert!(RuntimeSessionStatus::try_from("gone").is_err());
    assert!(RuntimeAttemptStatus::try_from("lost").is_err());
}
