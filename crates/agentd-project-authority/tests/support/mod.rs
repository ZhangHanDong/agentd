#![allow(dead_code)]

use std::collections::BTreeSet;

use agentd_core::ports::ProjectSnapshotResolveRequest;
use agentd_core::types::{
    AuthorityKey, CertificationGate, CertificationPolicyVersionRef, DataClassification,
    FrozenSpecVersionRef, MatrixRoomRef, OfflineRecoveryPolicy, OrganizationRef, PlacementPolicy,
    ProductWorkflowRef, ProjectExecutionSnapshot, ProjectExecutionSnapshotRef, ProjectRef,
    ProjectRoomBindingRef, QuotaPolicyVersionRef, RbacPolicyVersionRef, RepositoryBinding,
    RepositoryRef, RepositoryRole, RequirementRef, RoomBinding, RoomBindingRole, TeamRef,
};

pub fn authority(value: &str) -> AuthorityKey {
    AuthorityKey::new(value).expect("authority key")
}

pub fn snapshot(
    authority_key: AuthorityKey,
    project_id: &str,
    snapshot_id: &str,
    policy: OfflineRecoveryPolicy,
) -> ProjectExecutionSnapshot {
    let project_ref = ProjectRef::new(authority_key.clone(), project_id, "7").expect("project ref");
    let rbac_ref =
        RbacPolicyVersionRef::new(authority_key.clone(), "rbac-1", "4").expect("rbac ref");
    ProjectExecutionSnapshot {
        snapshot_ref: ProjectExecutionSnapshotRef::new(authority_key.clone(), snapshot_id, "9")
            .expect("snapshot ref"),
        authority_key: authority_key.clone(),
        authority_revision: 9,
        organization_ref: OrganizationRef::new(authority_key.clone(), "org-1", "2")
            .expect("organization ref"),
        team_refs: vec![
            TeamRef::new(authority_key.clone(), "team-runtime", "3").expect("team ref"),
        ],
        project_ref: project_ref.clone(),
        repository_bindings: vec![RepositoryBinding {
            repository_ref: RepositoryRef::new(authority_key.clone(), "repo-1", "5")
                .expect("repository ref"),
            role: RepositoryRole::Target,
            forge_locator: Some("github:corp/repo".to_string()),
            base_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
        }],
        room_bindings: vec![RoomBinding {
            binding_ref: ProjectRoomBindingRef::new(authority_key.clone(), "binding-1", "6")
                .expect("binding ref"),
            project_ref,
            matrix_room_ref: MatrixRoomRef::new(authority("matrix:corp"), "!room:corp", "11")
                .expect("matrix room ref"),
            roles: vec![RoomBindingRole::Command],
            allowed_command_classes: vec!["execute".to_string()],
            rbac_policy_version_ref: rbac_ref.clone(),
        }],
        issue_ref: None,
        requirement_refs: vec![
            RequirementRef::new(authority_key.clone(), "req-1", "8").expect("requirement ref"),
        ],
        frozen_spec_version_ref: FrozenSpecVersionRef::new(authority_key.clone(), "spec-1", "12")
            .expect("spec ref"),
        product_workflow_ref: ProductWorkflowRef::new(authority_key.clone(), "workflow-1", "13")
            .expect("workflow ref"),
        rbac_policy_version_ref: rbac_ref,
        quota_policy_version_ref: QuotaPolicyVersionRef::new(
            authority_key.clone(),
            "quota-1",
            "14",
        )
        .expect("quota ref"),
        certification_policy_version_ref: Some(
            CertificationPolicyVersionRef::new(authority_key, "cert-policy-1", "15")
                .expect("certification policy ref"),
        ),
        certification_gate: CertificationGate::Machine,
        skill_packages: Vec::new(),
        placement_policy: PlacementPolicy {
            data_classification: DataClassification::Restricted,
            allowed_regions: BTreeSet::from(["eu-west-1".to_string()]),
            allowed_worker_trust_domains: BTreeSet::from(["workers.example".to_string()]),
            require_signed_image: true,
            require_dedicated_pool: true,
            egress_profile_id: "restricted-egress-v1".to_string(),
            tenant_cache_namespace: format!("tenant/{project_id}"),
        },
        policy_revocation_epoch: 9,
        issued_at: 100,
        valid_until: 1_000,
        content_sha256: "a".repeat(64),
        offline_recovery_policy: policy,
    }
}

pub fn resolve_request(snapshot: &ProjectExecutionSnapshot) -> ProjectSnapshotResolveRequest {
    ProjectSnapshotResolveRequest {
        expected_authority: snapshot.authority_key.clone(),
        project_ref: snapshot.project_ref.clone(),
        requested_snapshot_ref: Some(snapshot.snapshot_ref.clone()),
    }
}
