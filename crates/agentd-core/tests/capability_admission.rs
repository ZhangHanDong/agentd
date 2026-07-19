use agentd_core::ports::{
    CapabilityAdmission, ExecutionSecurityScope, ProtectedAction, ProtectedResource, SecurityDenial,
};
use agentd_core::types::{
    AuthorityKey, FencingToken, LeaseId, OrganizationRef, ProjectExecutionSnapshotRef, ProjectRef,
    RbacPolicyVersionRef, TaskLeaseClaim, TaskRunId, WorkerIncarnationId,
};

fn admission(token: FencingToken) -> CapabilityAdmission {
    let authority = AuthorityKey::new("specify:corp").expect("authority");
    let worker = WorkerIncarnationId::new();
    CapabilityAdmission {
        action: ProtectedAction::SecretCheckout,
        resource: ProtectedResource::Secret("forge/token".into()),
        scope: ExecutionSecurityScope {
            authority_key: authority.clone(),
            organization_ref: OrganizationRef::new(authority.clone(), "org", "1")
                .expect("organization"),
            project_ref: ProjectRef::new(authority.clone(), "project", "1").expect("project"),
            snapshot_ref: ProjectExecutionSnapshotRef::new(authority.clone(), "snapshot", "1")
                .expect("snapshot"),
            rbac_policy_version_ref: RbacPolicyVersionRef::new(authority, "rbac", "1")
                .expect("rbac"),
            worker_incarnation_id: worker.clone(),
            lease_claim: TaskLeaseClaim {
                execution_task_id: TaskRunId::new(),
                worker_incarnation_id: worker,
                lease_id: LeaseId::new(),
                fencing_token: token,
            },
            sandbox_profile: "sha256:profile".into(),
            egress_profile: "none".into(),
            policy_revocation_epoch: 1,
            valid_from: 10,
            valid_until: 20,
        },
        fencing_token: token,
    }
}

#[test]
fn capability_rejects_outside_validity_window() {
    let capability = admission(FencingToken::new(1).expect("token"));
    assert_eq!(
        capability.validate_at(20),
        Err(SecurityDenial::CapabilityExpired)
    );
}

#[test]
fn capability_rejects_stale_fencing_token() {
    let mut capability = admission(FencingToken::new(1).expect("token"));
    capability.fencing_token = FencingToken::new(2).expect("token");
    assert_eq!(
        capability.validate_at(10),
        Err(SecurityDenial::LeaseRejected)
    );
}
