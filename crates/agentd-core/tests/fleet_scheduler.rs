use agentd_core::ports::{
    ArtifactUploadAckRequest, FleetCompletionReport, FleetDenialReason, FleetQueueStatus,
    FleetSideEffectRequest,
};
use agentd_core::types::{
    ArtifactUploadId, AuthenticatedWorkload, AuthorityKey, ExecutionArtifactId, FencingToken,
    LeaseId, OrganizationRef, ProjectExecutionSnapshotRef, ProjectRef, ProtectedAction,
    SecurityCheckpoint, TaskLeaseClaim, TaskRunId, WorkerId, WorkerIncarnationId, WorkloadRole,
};

fn incarnation() -> WorkerIncarnationId {
    WorkerIncarnationId::from_string("wi_01ARZ3NDEKTSV4RRFFQ69G5FAV")
}

fn workload() -> AuthenticatedWorkload {
    AuthenticatedWorkload {
        spiffe_uri: format!("spiffe://workers.example/worker/{}", incarnation()),
        role: WorkloadRole::Worker,
        trust_domain: "workers.example".to_string(),
        certificate_sha256: "a".repeat(64),
        not_before: 100,
        not_after: 300,
        worker_id: Some(WorkerId::from_string("wk_01ARZ3NDEKTSV4RRFFQ69G5FAW")),
        worker_incarnation_id: Some(incarnation()),
    }
}

fn claim() -> TaskLeaseClaim {
    TaskLeaseClaim {
        execution_task_id: TaskRunId::from_string("tr_01ARZ3NDEKTSV4RRFFQ69G5FAX"),
        worker_incarnation_id: incarnation(),
        lease_id: LeaseId::from_string("ls_01ARZ3NDEKTSV4RRFFQ69G5FAY"),
        fencing_token: FencingToken::new(7).expect("fencing"),
    }
}

#[test]
fn fleet_reports_bind_canonical_task_worker_lease_and_fencing_identity() {
    let completion = FleetCompletionReport {
        workload: workload(),
        claim: claim(),
        idempotency_key: "completion-1".to_string(),
        outcome_sha256: "b".repeat(64),
        observed_at: 200,
    };
    let upload = ArtifactUploadAckRequest {
        workload: workload(),
        claim: claim(),
        upload_id: ArtifactUploadId::from_string("au_01ARZ3NDEKTSV4RRFFQ69G5FAZ"),
        execution_artifact_id: ExecutionArtifactId::from_string("ar_01ARZ3NDEKTSV4RRFFQ69G5FB0"),
        idempotency_key: "upload-1".to_string(),
        artifact_sha256: "c".repeat(64),
        upload_attempt: 1,
        part_count: 3,
        observed_at: 210,
    };
    let side_effect = FleetSideEffectRequest {
        workload: workload(),
        claim: claim(),
        checkpoint: SecurityCheckpoint::Delivery,
        action: ProtectedAction::SandboxExecute,
        idempotency_key: "delivery-1".to_string(),
        observed_at: 220,
    };

    assert_eq!(completion.claim, upload.claim);
    assert_eq!(upload.claim, side_effect.claim);
    assert_eq!(side_effect.claim.fencing_token.value(), 7);
    assert!(FleetQueueStatus::DeadLetter.is_terminal());
    assert_eq!(
        FleetDenialReason::StaleFencingToken.as_str(),
        "stale_fencing_token"
    );
}

#[test]
fn authority_refs_remain_distinct_from_scheduler_identity() {
    let authority = AuthorityKey::new("specify:fleet-test").expect("authority");
    let organization = OrganizationRef::new(authority.clone(), "org-a", "1").expect("org");
    let project = ProjectRef::new(authority.clone(), "project-a", "2").expect("project");
    let snapshot =
        ProjectExecutionSnapshotRef::new(authority, "snapshot-a", "3").expect("snapshot");

    assert_ne!(organization.resource_id(), project.resource_id());
    assert_ne!(project.resource_id(), snapshot.resource_id());
    assert_ne!(claim().execution_task_id.as_str(), snapshot.resource_id());
}
