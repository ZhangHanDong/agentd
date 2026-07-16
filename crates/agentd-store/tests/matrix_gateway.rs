use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use agentd_core::ports::{
    MatrixCommandClass, MatrixCommandDisposition, MatrixGatewayCommandRequest,
    MatrixGatewayCutoverRequest, MatrixGatewayError, MatrixGatewayMappingKind, MatrixGatewayMode,
    MatrixGatewayPort, MatrixGatewayProjectConfig, MatrixGatewayStateMappingRequest,
    MatrixTransportProvenance, NormalizedMatrixCommand, PolicyRevocationPort, SecurityError,
};
use agentd_core::types::{
    AuthorityKey, CertificationGate, CertificationPolicyVersionRef, DataClassification,
    EnterpriseRequestIdentity, FrozenSpecVersionRef, MatrixPrincipalResolveRequest, MatrixRoomRef,
    MatrixTrustPolicy, OfflineRecoveryPolicy, OrganizationRef, PlacementPolicy, PrincipalKind,
    ProductWorkflowRef, ProjectExecutionSnapshot, ProjectExecutionSnapshotRef, ProjectRef,
    ProjectRoomBindingRef, QuotaPolicyVersionRef, RbacPolicyVersionRef, RepositoryBinding,
    RepositoryRef, RepositoryRole, RoomBinding, RoomBindingRole, SecurityEpochRequest,
    SecurityEpochStatus, TeamRef,
};
use agentd_store::principal_repo::{PrincipalUpsert, SqliteEnterprisePrincipalRepository};
use agentd_store::{SqliteStore, matrix_gateway::SqliteMatrixGateway};

struct Fixture {
    _dir: tempfile::TempDir,
    store: SqliteStore,
    gateway: SqliteMatrixGateway,
    snapshot: ProjectExecutionSnapshot,
    identity: EnterpriseRequestIdentity,
    current_epoch: Arc<AtomicU64>,
}

#[derive(Debug)]
struct CurrentEpoch(Arc<AtomicU64>);

#[async_trait::async_trait]
impl PolicyRevocationPort for CurrentEpoch {
    async fn check_security_epoch(
        &self,
        request: &SecurityEpochRequest,
    ) -> Result<SecurityEpochStatus, SecurityError> {
        Ok(SecurityEpochStatus {
            checkpoint: request.checkpoint,
            organization_ref: request.organization_ref.clone(),
            project_ref: request.project_ref.clone(),
            execution_snapshot_ref: request.execution_snapshot_ref.clone(),
            current_epoch: self.0.load(Ordering::SeqCst),
            observed_at: request.observed_at,
        })
    }
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let snapshot = snapshot();
    let principal_repo = SqliteEnterprisePrincipalRepository::new(
        store.pool().clone(),
        MatrixTrustPolicy {
            trusted_homeservers: BTreeSet::from(["matrix.example".to_string()]),
            trusted_appservices: BTreeSet::from(["agentd-appservice".to_string()]),
        },
        600,
    )
    .expect("principal repository");
    let principal = principal_repo
        .upsert_principal(PrincipalUpsert {
            id: agentd_core::types::EnterprisePrincipalId::new(),
            organization_ref: snapshot.organization_ref.clone(),
            kind: PrincipalKind::Human,
            display_name: "Matrix Operator".to_string(),
            observed_at: 100,
        })
        .await
        .expect("principal");
    let identity = EnterpriseRequestIdentity::matrix(
        principal,
        MatrixPrincipalResolveRequest {
            user_id: "@operator:matrix.example".to_string(),
            homeserver: "matrix.example".to_string(),
            device_id: Some("DEVICE-A".to_string()),
            appservice_id: None,
            observed_at: 120,
        },
        900,
    );
    let current_epoch = Arc::new(AtomicU64::new(snapshot.policy_revocation_epoch));
    let gateway = SqliteMatrixGateway::new(
        store.pool().clone(),
        Arc::new(CurrentEpoch(Arc::clone(&current_epoch))),
    );
    Fixture {
        _dir: dir,
        store,
        gateway,
        snapshot,
        identity,
        current_epoch,
    }
}

fn authority() -> AuthorityKey {
    AuthorityKey::new("specify:matrix-gateway-test").expect("authority")
}

fn snapshot() -> ProjectExecutionSnapshot {
    let authority = authority();
    let project = ProjectRef::new(authority.clone(), "project-a", "2").expect("project");
    let rbac = RbacPolicyVersionRef::new(authority.clone(), "rbac-a", "3").expect("rbac");
    ProjectExecutionSnapshot {
        snapshot_ref: ProjectExecutionSnapshotRef::new(authority.clone(), "snapshot-a", "4")
            .expect("snapshot"),
        authority_key: authority.clone(),
        authority_revision: 4,
        organization_ref: OrganizationRef::new(authority.clone(), "org-a", "1").expect("org"),
        team_refs: vec![TeamRef::new(authority.clone(), "team-a", "1").expect("team")],
        project_ref: project.clone(),
        repository_bindings: vec![RepositoryBinding {
            repository_ref: RepositoryRef::new(authority.clone(), "repo-a", "1").expect("repo"),
            role: RepositoryRole::Target,
            forge_locator: Some("github:org/repo".to_string()),
            base_commit: "a".repeat(40),
        }],
        room_bindings: vec![RoomBinding {
            binding_ref: ProjectRoomBindingRef::new(authority.clone(), "room-binding-a", "1")
                .expect("binding"),
            project_ref: project,
            matrix_room_ref: MatrixRoomRef::new(
                AuthorityKey::new("matrix:matrix-gateway-test").expect("matrix authority"),
                "!project-a:matrix.example",
                "1",
            )
            .expect("room"),
            roles: vec![RoomBindingRole::Command],
            allowed_command_classes: vec![
                "execute".to_string(),
                "status".to_string(),
                "cancel".to_string(),
            ],
            rbac_policy_version_ref: rbac.clone(),
        }],
        issue_ref: None,
        requirement_refs: Vec::new(),
        frozen_spec_version_ref: FrozenSpecVersionRef::new(authority.clone(), "spec-a", "1")
            .expect("spec"),
        product_workflow_ref: ProductWorkflowRef::new(authority.clone(), "workflow-a", "1")
            .expect("workflow"),
        rbac_policy_version_ref: rbac,
        quota_policy_version_ref: QuotaPolicyVersionRef::new(authority.clone(), "quota-a", "1")
            .expect("quota"),
        certification_policy_version_ref: Some(
            CertificationPolicyVersionRef::new(authority, "cert-a", "1").expect("cert"),
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
            tenant_cache_namespace: "org-a/project-a".to_string(),
        },
        policy_revocation_epoch: 9,
        issued_at: 100,
        valid_until: 1_000,
        content_sha256: "b".repeat(64),
        offline_recovery_policy: OfflineRecoveryPolicy::Deny,
    }
}

async fn configure(fixture: &Fixture) {
    fixture
        .gateway
        .configure_project(&MatrixGatewayProjectConfig {
            binding_ref: fixture.snapshot.room_bindings[0].binding_ref.clone(),
            snapshot: fixture.snapshot.clone(),
            room_id: "!project-a:matrix.example".to_string(),
            mode: MatrixGatewayMode::Observe,
            trusted_inviters: vec!["@admin:matrix.example".to_string()],
            ignored_senders: vec!["@ignored:matrix.example".to_string()],
            gateway_user_id: "@agentd:matrix.example".to_string(),
            configured_at: 120,
        })
        .await
        .expect("configure gateway");
}

fn command_request(
    fixture: &Fixture,
    event_id: &str,
    previous_cursor: &str,
    cursor: &str,
    observed_at: i64,
) -> MatrixGatewayCommandRequest {
    MatrixGatewayCommandRequest {
        provenance: MatrixTransportProvenance {
            event_id: event_id.to_string(),
            room_id: "!project-a:matrix.example".to_string(),
            sender_user_id: "@operator:matrix.example".to_string(),
            homeserver: "matrix.example".to_string(),
            device_id: Some("DEVICE-A".to_string()),
            appservice_id: None,
            authenticated_sender_user_id: "@operator:matrix.example".to_string(),
            authenticated_appservice_id: None,
            inviter_user_id: Some("@admin:matrix.example".to_string()),
            origin_server_ts: observed_at - 1,
            transport_authenticated: true,
            previous_sync_cursor: previous_cursor.to_string(),
            sync_cursor: cursor.to_string(),
        },
        identity: fixture.identity.clone(),
        binding_ref: fixture.snapshot.room_bindings[0].binding_ref.clone(),
        snapshot_ref: fixture.snapshot.snapshot_ref.clone(),
        command: NormalizedMatrixCommand {
            class: MatrixCommandClass::Execute,
            arguments: vec!["spec:immutable".to_string()],
            attachments: Vec::new(),
            command_sha256: "c".repeat(64),
        },
        observed_at,
    }
}

async fn transition(
    fixture: &Fixture,
    expected_mode: MatrixGatewayMode,
    next_mode: MatrixGatewayMode,
    cursor: &str,
    observed_at: i64,
) {
    fixture
        .gateway
        .transition_cutover(&MatrixGatewayCutoverRequest {
            binding_ref: fixture.snapshot.room_bindings[0].binding_ref.clone(),
            expected_mode,
            next_mode,
            cursor: cursor.to_string(),
            reason_code: "operator-approved".to_string(),
            observed_at,
        })
        .await
        .expect("transition cutover");
}

async fn count(fixture: &Fixture, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(fixture.store.pool())
        .await
        .expect("count")
}

#[tokio::test]
async fn observe_shadow_canary_handoff_and_replay_are_atomic_and_idempotent() {
    let fixture = fixture().await;
    configure(&fixture).await;

    let observed = fixture
        .gateway
        .accept_command(&command_request(&fixture, "$observe", "", "s1", 200))
        .await
        .expect("observe");
    assert_eq!(observed.disposition, MatrixCommandDisposition::Observed);
    assert!(observed.run_id.is_none());
    assert!(observed.outbox_id.is_none());

    transition(
        &fixture,
        MatrixGatewayMode::Observe,
        MatrixGatewayMode::ShadowReadOnly,
        "s1",
        210,
    )
    .await;
    let shadowed = fixture
        .gateway
        .accept_command(&command_request(&fixture, "$shadow", "s1", "s2", 220))
        .await
        .expect("shadow");
    assert_eq!(shadowed.disposition, MatrixCommandDisposition::Shadowed);
    assert!(shadowed.run_id.is_none());
    assert!(shadowed.outbox_id.is_none());

    transition(
        &fixture,
        MatrixGatewayMode::ShadowReadOnly,
        MatrixGatewayMode::Canary,
        "s2",
        230,
    )
    .await;
    let request = command_request(&fixture, "$canary", "s2", "s3", 240);
    let accepted = fixture
        .gateway
        .accept_command(&request)
        .await
        .expect("canary");
    assert_eq!(accepted.disposition, MatrixCommandDisposition::Accepted);
    assert!(accepted.run_id.is_some());
    assert!(accepted.outbox_id.is_some());

    let replay = fixture
        .gateway
        .accept_command(&request)
        .await
        .expect("replay");
    assert_eq!(replay.disposition, MatrixCommandDisposition::Replayed);
    assert_eq!(replay.command_id, accepted.command_id);
    assert_eq!(replay.run_id, accepted.run_id);
    assert_eq!(replay.outbox_id, accepted.outbox_id);
    let pending = fixture
        .gateway
        .outbox_after(None, 10)
        .await
        .expect("pending outbox");
    assert_eq!(pending.len(), 1);
    let delivered = fixture
        .gateway
        .mark_outbox_delivered(&pending[0].outbox_id, 250)
        .await
        .expect("delivery ack");
    assert_eq!(delivered.delivered_at, Some(250));
    assert_eq!(
        fixture
            .gateway
            .mark_outbox_delivered(&pending[0].outbox_id, 260)
            .await
            .expect("idempotent delivery ack"),
        delivered
    );
    assert!(
        fixture
            .gateway
            .outbox_after(None, 10)
            .await
            .expect("drained outbox")
            .is_empty()
    );
    assert_eq!(count(&fixture, "matrix_gateway_commands").await, 3);
    assert_eq!(count(&fixture, "matrix_gateway_inbox").await, 3);
    assert_eq!(count(&fixture, "runs").await, 1);
    assert_eq!(count(&fixture, "matrix_gateway_outbox").await, 1);
    let view = fixture
        .gateway
        .project_view(&fixture.snapshot.room_bindings[0].binding_ref, 10)
        .await
        .expect("view")
        .expect("configured view");
    assert_eq!(view.mode, MatrixGatewayMode::Canary);
    assert_eq!(view.sync_cursor, "s3");
    assert_eq!(view.recent_runs.len(), 1);
    assert_eq!(view.recent_runs[0].run_id, accepted.run_id.expect("run id"));
}

#[tokio::test]
async fn cursor_conflict_denial_and_rollback_never_create_execution_side_effects() {
    let fixture = fixture().await;
    configure(&fixture).await;
    for (index, (kind, canonical_ref, in_flight)) in [
        (
            MatrixGatewayMappingKind::Project,
            "specify:project/project-a@2",
            false,
        ),
        (
            MatrixGatewayMappingKind::Room,
            "matrix:room/!project-a@1",
            false,
        ),
        (
            MatrixGatewayMappingKind::Principal,
            "agentd:principal/operator",
            false,
        ),
        (MatrixGatewayMappingKind::Task, "agentd:task/tr_01", true),
        (
            MatrixGatewayMappingKind::Message,
            "agentd:command/mc_01",
            false,
        ),
        (
            MatrixGatewayMappingKind::Cursor,
            "agentd:cursor/initial",
            false,
        ),
        (MatrixGatewayMappingKind::Run, "agentd:run/r_01", true),
    ]
    .into_iter()
    .enumerate()
    {
        let request = MatrixGatewayStateMappingRequest {
            binding_ref: fixture.snapshot.room_bindings[0].binding_ref.clone(),
            kind,
            legacy_ref_sha256: format!("{:064x}", index + 1),
            canonical_ref: canonical_ref.to_string(),
            in_flight,
            observed_at: 190,
        };
        let first = fixture
            .gateway
            .record_state_mapping(&request)
            .await
            .expect("state mapping");
        assert_eq!(
            fixture
                .gateway
                .record_state_mapping(&request)
                .await
                .expect("idempotent state mapping"),
            first
        );
    }
    transition(
        &fixture,
        MatrixGatewayMode::Observe,
        MatrixGatewayMode::ShadowReadOnly,
        "",
        200,
    )
    .await;
    transition(
        &fixture,
        MatrixGatewayMode::ShadowReadOnly,
        MatrixGatewayMode::Canary,
        "",
        210,
    )
    .await;

    let conflict = fixture
        .gateway
        .accept_command(&command_request(&fixture, "$stale", "stale", "s1", 220))
        .await
        .expect_err("stale cursor");
    assert!(matches!(conflict, MatrixGatewayError::Conflict(_)));
    assert_eq!(count(&fixture, "matrix_gateway_commands").await, 0);
    assert_eq!(count(&fixture, "runs").await, 0);

    let accepted = fixture
        .gateway
        .accept_command(&command_request(&fixture, "$accepted", "", "s1", 230))
        .await
        .expect("accepted");
    assert!(accepted.run_id.is_some());
    transition(
        &fixture,
        MatrixGatewayMode::Canary,
        MatrixGatewayMode::RolledBack,
        "s1",
        240,
    )
    .await;
    let ignored = fixture
        .gateway
        .accept_command(&command_request(&fixture, "$rolled-back", "s1", "s2", 250))
        .await
        .expect("rolled back command ledger");
    assert_eq!(ignored.disposition, MatrixCommandDisposition::Ignored);
    assert!(ignored.run_id.is_none());
    assert!(ignored.outbox_id.is_none());
    assert_eq!(count(&fixture, "runs").await, 1);
    assert_eq!(count(&fixture, "matrix_gateway_outbox").await, 1);
    assert_eq!(count(&fixture, "matrix_gateway_cutover_history").await, 3);
    let manifest = fixture
        .gateway
        .rollback_manifest(&fixture.snapshot.room_bindings[0].binding_ref)
        .await
        .expect("rollback manifest");
    assert_eq!(manifest.mode, MatrixGatewayMode::RolledBack);
    assert_eq!(manifest.current_cursor, "s1");
    assert_eq!(manifest.mappings.len(), 7);
    assert_eq!(
        manifest
            .mappings
            .iter()
            .filter(|mapping| mapping.in_flight)
            .count(),
        2
    );
}

#[tokio::test]
async fn sender_inviter_appservice_and_binding_denials_leave_no_gateway_ledger() {
    let fixture = fixture().await;
    configure(&fixture).await;

    let mut ignored = command_request(&fixture, "$ignored", "", "s1", 200);
    ignored.provenance.sender_user_id = "@ignored:matrix.example".to_string();
    ignored.provenance.authenticated_sender_user_id = "@ignored:matrix.example".to_string();
    assert!(matches!(
        fixture.gateway.accept_command(&ignored).await,
        Err(MatrixGatewayError::Denied(
            agentd_core::ports::MatrixGatewayDenialReason::SenderIgnored
        ))
    ));

    let mut inviter = command_request(&fixture, "$inviter", "", "s1", 201);
    inviter.provenance.inviter_user_id = Some("@unknown:matrix.example".to_string());
    assert!(matches!(
        fixture.gateway.accept_command(&inviter).await,
        Err(MatrixGatewayError::Denied(
            agentd_core::ports::MatrixGatewayDenialReason::InviterUntrusted
        ))
    ));

    let mut looped = command_request(&fixture, "$loop", "", "s1", 202);
    looped.provenance.sender_user_id = "@agentd:matrix.example".to_string();
    looped.provenance.authenticated_sender_user_id = "@agentd:matrix.example".to_string();
    assert!(matches!(
        fixture.gateway.accept_command(&looped).await,
        Err(MatrixGatewayError::Denied(
            agentd_core::ports::MatrixGatewayDenialReason::AppserviceLoop
        ))
    ));

    let mut foreign_room = command_request(&fixture, "$foreign", "", "s1", 203);
    foreign_room.provenance.room_id = "!foreign:matrix.example".to_string();
    assert!(matches!(
        fixture.gateway.accept_command(&foreign_room).await,
        Err(MatrixGatewayError::Denied(
            agentd_core::ports::MatrixGatewayDenialReason::BindingMismatch
        ))
    ));
    assert_eq!(count(&fixture, "matrix_gateway_commands").await, 0);
    assert_eq!(count(&fixture, "matrix_gateway_inbox").await, 0);
    assert_eq!(count(&fixture, "runs").await, 0);
    assert_eq!(count(&fixture, "matrix_gateway_outbox").await, 0);
}

#[tokio::test]
async fn revoked_project_epoch_and_disabled_principal_are_rechecked_inside_handoff() {
    let fixture = fixture().await;
    configure(&fixture).await;
    fixture.current_epoch.store(10, Ordering::SeqCst);
    assert!(matches!(
        fixture
            .gateway
            .accept_command(&command_request(&fixture, "$revoked", "", "s1", 200))
            .await,
        Err(MatrixGatewayError::Denied(
            agentd_core::ports::MatrixGatewayDenialReason::ProjectAuthorizationStale
        ))
    ));
    assert_eq!(count(&fixture, "matrix_gateway_commands").await, 0);

    fixture.current_epoch.store(9, Ordering::SeqCst);
    sqlx::query(
        "UPDATE enterprise_principals SET status = 'disabled', disabled_at = 201 WHERE id = ?",
    )
    .bind(fixture.identity.principal.id.as_str())
    .execute(fixture.store.pool())
    .await
    .expect("disable principal");
    assert!(matches!(
        fixture
            .gateway
            .accept_command(&command_request(&fixture, "$disabled", "", "s1", 202))
            .await,
        Err(MatrixGatewayError::Denied(
            agentd_core::ports::MatrixGatewayDenialReason::PrincipalUnauthorized
        ))
    ));
    assert_eq!(count(&fixture, "matrix_gateway_commands").await, 0);
    assert_eq!(count(&fixture, "runs").await, 0);
}

#[tokio::test]
async fn native_gateway_schema_stores_content_references_not_raw_matrix_transcripts() {
    let fixture = fixture().await;
    configure(&fixture).await;
    fixture
        .gateway
        .accept_command(&command_request(&fixture, "$observed", "", "s1", 200))
        .await
        .expect("observed command");

    let schema: String = sqlx::query_scalar(
        "SELECT group_concat(sql, ' ') FROM sqlite_master \
         WHERE name LIKE 'matrix_gateway_%' AND type IN ('table', 'index', 'trigger')",
    )
    .fetch_one(fixture.store.pool())
    .await
    .expect("gateway schema");
    let normalized = schema.to_ascii_lowercase();
    for forbidden in [
        "raw_body",
        "transcript",
        "attachment_bytes",
        "worktree_path",
    ] {
        assert!(
            !normalized.contains(forbidden),
            "stored forbidden field {forbidden}"
        );
    }
    let arguments_sha256: String = sqlx::query_scalar(
        "SELECT arguments_sha256 FROM matrix_gateway_commands WHERE event_id = '$observed'",
    )
    .fetch_one(fixture.store.pool())
    .await
    .expect("arguments digest");
    assert_eq!(arguments_sha256.len(), 64);
}
