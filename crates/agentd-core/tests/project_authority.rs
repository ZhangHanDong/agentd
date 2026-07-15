use agentd_core::types::{
    AuthorityKey, AuthorityResourceRef, CertificationPolicyVersionRef, FrozenSpecVersionRef,
    MatrixRoomRef, OfflineRecoveryPolicy, OrganizationRef, ProductWorkflowRef,
    ProjectExecutionSnapshot, ProjectExecutionSnapshotRef, ProjectRef, ProjectRoomBindingRef,
    QuotaPolicyVersionRef, RbacPolicyVersionRef, RepositoryBinding, RepositoryRef, RepositoryRole,
    RequirementRef, ResourceKind, RoomBinding, RoomBindingRole, TeamRef,
};

fn authority(value: &str) -> AuthorityKey {
    AuthorityKey::new(value).expect("authority key")
}

fn valid_snapshot() -> ProjectExecutionSnapshot {
    let project_authority = authority("specify:corp");
    let project_ref =
        ProjectRef::new(project_authority.clone(), "project-1", "7").expect("project ref");
    let rbac_ref =
        RbacPolicyVersionRef::new(project_authority.clone(), "rbac-1", "4").expect("rbac ref");
    ProjectExecutionSnapshot {
        snapshot_ref: ProjectExecutionSnapshotRef::new(
            project_authority.clone(),
            "snapshot-42",
            "9",
        )
        .expect("snapshot ref"),
        authority_key: project_authority.clone(),
        authority_revision: 9,
        organization_ref: OrganizationRef::new(project_authority.clone(), "org-1", "2")
            .expect("organization ref"),
        team_refs: vec![
            TeamRef::new(project_authority.clone(), "team-runtime", "3").expect("team ref"),
        ],
        project_ref: project_ref.clone(),
        repository_bindings: vec![RepositoryBinding {
            repository_ref: RepositoryRef::new(project_authority.clone(), "repo-1", "5")
                .expect("repository ref"),
            role: RepositoryRole::Target,
            forge_locator: Some("github:corp/repo".to_string()),
            base_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
        }],
        room_bindings: vec![RoomBinding {
            binding_ref: ProjectRoomBindingRef::new(project_authority.clone(), "binding-1", "6")
                .expect("binding ref"),
            project_ref,
            matrix_room_ref: MatrixRoomRef::new(authority("matrix:corp"), "!room:corp", "11")
                .expect("matrix room ref"),
            roles: vec![RoomBindingRole::Command, RoomBindingRole::Notification],
            allowed_command_classes: vec!["execute".to_string()],
            rbac_policy_version_ref: rbac_ref.clone(),
        }],
        issue_ref: None,
        requirement_refs: vec![
            RequirementRef::new(project_authority.clone(), "req-1", "8").expect("requirement ref"),
        ],
        frozen_spec_version_ref: FrozenSpecVersionRef::new(
            project_authority.clone(),
            "spec-1",
            "12",
        )
        .expect("frozen spec ref"),
        product_workflow_ref: ProductWorkflowRef::new(
            project_authority.clone(),
            "workflow-1",
            "13",
        )
        .expect("workflow ref"),
        rbac_policy_version_ref: rbac_ref,
        quota_policy_version_ref: QuotaPolicyVersionRef::new(
            project_authority.clone(),
            "quota-1",
            "14",
        )
        .expect("quota ref"),
        certification_policy_version_ref: Some(
            CertificationPolicyVersionRef::new(project_authority, "cert-policy-1", "15")
                .expect("certification policy ref"),
        ),
        issued_at: 100,
        valid_until: 1_000,
        content_sha256: "a".repeat(64),
        offline_recovery_policy: OfflineRecoveryPolicy::AllowPinnedUntilExpiry,
    }
}

#[test]
fn project_authority_refs_and_snapshot_validation_follow_p266() {
    let first = AuthorityResourceRef::new(
        authority("specify:corp"),
        ResourceKind::Project,
        "same-id",
        "1",
    )
    .expect("first ref");
    let other_authority = AuthorityResourceRef::new(
        authority("local:standalone"),
        ResourceKind::Project,
        "same-id",
        "1",
    )
    .expect("other authority ref");
    let other_kind = AuthorityResourceRef::new(
        authority("specify:corp"),
        ResourceKind::Organization,
        "same-id",
        "1",
    )
    .expect("other kind ref");
    let other_version = AuthorityResourceRef::new(
        authority("specify:corp"),
        ResourceKind::Project,
        "same-id",
        "2",
    )
    .expect("other version ref");
    assert_ne!(first, other_authority);
    assert_ne!(first, other_kind);
    assert_ne!(first, other_version);
    assert!(ProjectRef::try_from(other_kind).is_err());
    assert!(
        AuthorityResourceRef::new(
            authority("specify:corp"),
            ResourceKind::Project,
            "latest",
            "1"
        )
        .is_err()
    );

    let snapshot = valid_snapshot();
    snapshot.validate().expect("valid P266 snapshot");
    let target = snapshot.target_repository().expect("target repository");
    assert_eq!(target.role, RepositoryRole::Target);
    assert_eq!(target.repository_ref.resource_id(), "repo-1");

    let mut wrong_authority = snapshot.clone();
    wrong_authority.quota_policy_version_ref =
        QuotaPolicyVersionRef::new(authority("specify:other"), "quota-1", "14")
            .expect("other quota ref");
    assert!(wrong_authority.validate().is_err());

    let mut bad_hash = snapshot.clone();
    bad_hash.content_sha256 = "BAD".to_string();
    assert!(bad_hash.validate().is_err());

    let mut bad_commit = snapshot.clone();
    bad_commit.repository_bindings[0].base_commit = "main".to_string();
    assert!(bad_commit.validate().is_err());

    let mut two_targets = snapshot;
    two_targets
        .repository_bindings
        .push(two_targets.repository_bindings[0].clone());
    assert!(two_targets.validate().is_err());
}
