use std::sync::{Arc, Mutex};

use agentd_bin::matrix_gateway::{
    MatrixGatewayProviderKind, MatrixGatewayProviders, MatrixInboundAttachment,
    MatrixInboundTransportEvent, build_agentd_matrix_gateway, normalize_command,
};
use agentd_core::ports::{
    Clock, MatrixCommandClass, MatrixCommandDisposition, MatrixCommandReceipt,
    MatrixGatewayCommandRequest, MatrixGatewayCutoverRequest, MatrixGatewayDeliveryPort,
    MatrixGatewayDenialReason, MatrixGatewayError, MatrixGatewayIdentityPort, MatrixGatewayMode,
    MatrixGatewayOutboxRecord, MatrixGatewayPort, MatrixGatewayProjectConfig,
    MatrixGatewayRollbackManifest, MatrixGatewayStateMapping, MatrixGatewayStateMappingRequest,
    MatrixGatewaySummaryPublish, MatrixTransportProvenance, RobrixProjectView,
};
use agentd_core::types::{
    AuthorityKey, EnterprisePrincipal, EnterprisePrincipalId, EnterpriseRequestIdentity,
    MatrixCommandId, MatrixGatewayOutboxId, MatrixPrincipalResolveRequest, OrganizationRef,
    PrincipalKind, PrincipalStatus, ProjectExecutionSnapshotRef, ProjectRoomBindingRef,
};

#[derive(Debug)]
struct FixedClock(i64);

impl Clock for FixedClock {
    fn now_unix(&self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone)]
struct AuthenticatingIdentity {
    identity: EnterpriseRequestIdentity,
}

#[async_trait::async_trait]
impl MatrixGatewayIdentityPort for AuthenticatingIdentity {
    async fn authenticate_matrix_source(
        &self,
        provenance: &MatrixTransportProvenance,
    ) -> Result<EnterpriseRequestIdentity, MatrixGatewayError> {
        if !provenance.transport_authenticated
            || provenance.sender_user_id != provenance.authenticated_sender_user_id
        {
            return Err(MatrixGatewayError::Denied(
                MatrixGatewayDenialReason::TransportIdentityMismatch,
            ));
        }
        Ok(self.identity.clone())
    }
}

#[derive(Debug, Default)]
struct RecordingGateway {
    requests: Mutex<Vec<MatrixGatewayCommandRequest>>,
    outbox: Mutex<Vec<MatrixGatewayOutboxRecord>>,
}

#[async_trait::async_trait]
impl MatrixGatewayPort for RecordingGateway {
    async fn configure_project(
        &self,
        _config: &MatrixGatewayProjectConfig,
    ) -> Result<RobrixProjectView, MatrixGatewayError> {
        Err(MatrixGatewayError::Unavailable("unused".to_string()))
    }

    async fn accept_command(
        &self,
        request: &MatrixGatewayCommandRequest,
    ) -> Result<MatrixCommandReceipt, MatrixGatewayError> {
        self.requests
            .lock()
            .expect("recording gateway lock")
            .push(request.clone());
        Ok(MatrixCommandReceipt {
            command_id: MatrixCommandId::new(),
            event_id: request.provenance.event_id.clone(),
            disposition: MatrixCommandDisposition::Accepted,
            run_id: None,
            outbox_id: Some(MatrixGatewayOutboxId::new()),
            mode: MatrixGatewayMode::Canary,
            reason_code: None,
            accepted_at: request.observed_at,
        })
    }

    async fn transition_cutover(
        &self,
        _request: &MatrixGatewayCutoverRequest,
    ) -> Result<RobrixProjectView, MatrixGatewayError> {
        Err(MatrixGatewayError::Unavailable("unused".to_string()))
    }

    async fn record_state_mapping(
        &self,
        _request: &MatrixGatewayStateMappingRequest,
    ) -> Result<MatrixGatewayStateMapping, MatrixGatewayError> {
        Err(MatrixGatewayError::Unavailable("unused".to_string()))
    }

    async fn rollback_manifest(
        &self,
        _binding_ref: &ProjectRoomBindingRef,
    ) -> Result<MatrixGatewayRollbackManifest, MatrixGatewayError> {
        Err(MatrixGatewayError::Unavailable("unused".to_string()))
    }

    async fn publish_summary(
        &self,
        _request: &MatrixGatewaySummaryPublish,
    ) -> Result<MatrixGatewayOutboxId, MatrixGatewayError> {
        Err(MatrixGatewayError::Unavailable("unused".to_string()))
    }

    async fn outbox_after(
        &self,
        _after_sequence: Option<u64>,
        _limit: u32,
    ) -> Result<Vec<MatrixGatewayOutboxRecord>, MatrixGatewayError> {
        Ok(self
            .outbox
            .lock()
            .expect("outbox")
            .iter()
            .filter(|record| record.delivered_at.is_none())
            .cloned()
            .collect())
    }

    async fn mark_outbox_delivered(
        &self,
        outbox_id: &MatrixGatewayOutboxId,
        delivered_at: i64,
    ) -> Result<MatrixGatewayOutboxRecord, MatrixGatewayError> {
        let mut records = self.outbox.lock().expect("outbox");
        let record = records
            .iter_mut()
            .find(|record| &record.outbox_id == outbox_id)
            .ok_or_else(|| MatrixGatewayError::NotFound("outbox".to_string()))?;
        record.delivered_at.get_or_insert(delivered_at);
        Ok(record.clone())
    }

    async fn project_view(
        &self,
        _binding_ref: &ProjectRoomBindingRef,
        _recent_limit: u32,
    ) -> Result<Option<RobrixProjectView>, MatrixGatewayError> {
        Err(MatrixGatewayError::Unavailable("unused".to_string()))
    }
}

#[derive(Debug, Default)]
struct RecordingDelivery {
    outbox_ids: Mutex<Vec<MatrixGatewayOutboxId>>,
}

#[async_trait::async_trait]
impl MatrixGatewayDeliveryPort for RecordingDelivery {
    async fn deliver_summary(
        &self,
        record: &MatrixGatewayOutboxRecord,
    ) -> Result<(), MatrixGatewayError> {
        self.outbox_ids
            .lock()
            .expect("delivery")
            .push(record.outbox_id.clone());
        Ok(())
    }
}

fn authority() -> AuthorityKey {
    AuthorityKey::new("specify:matrix-bin-test").expect("authority")
}

fn identity() -> EnterpriseRequestIdentity {
    let authority = authority();
    EnterpriseRequestIdentity::matrix(
        EnterprisePrincipal {
            id: EnterprisePrincipalId::new(),
            organization_ref: OrganizationRef::new(authority, "org-a", "1").expect("organization"),
            kind: PrincipalKind::Human,
            status: PrincipalStatus::Active,
            display_name: "Operator".to_string(),
            created_at: 100,
            updated_at: 100,
            disabled_at: None,
        },
        MatrixPrincipalResolveRequest {
            user_id: "@operator:matrix.example".to_string(),
            homeserver: "matrix.example".to_string(),
            device_id: Some("DEVICE-A".to_string()),
            appservice_id: None,
            observed_at: 200,
        },
        900,
    )
}

fn event(sender: &str, authenticated_sender: &str) -> MatrixInboundTransportEvent {
    MatrixInboundTransportEvent {
        provenance: MatrixTransportProvenance {
            event_id: "$event-a".to_string(),
            room_id: "!project-a:matrix.example".to_string(),
            sender_user_id: sender.to_string(),
            homeserver: "matrix.example".to_string(),
            device_id: Some("DEVICE-A".to_string()),
            appservice_id: None,
            authenticated_sender_user_id: authenticated_sender.to_string(),
            authenticated_appservice_id: None,
            inviter_user_id: Some("@admin:matrix.example".to_string()),
            origin_server_ts: 199,
            transport_authenticated: true,
            previous_sync_cursor: "s0".to_string(),
            sync_cursor: "s1".to_string(),
        },
        binding_ref: ProjectRoomBindingRef::new(authority(), "binding-a", "1").expect("binding"),
        snapshot_ref: ProjectExecutionSnapshotRef::new(authority(), "snapshot-a", "1")
            .expect("snapshot"),
        body: "/agentd execute spec:immutable".to_string(),
        attachments: vec![MatrixInboundAttachment {
            bytes: b"content-addressed input".to_vec(),
            media_type: "text/plain".to_string(),
        }],
    }
}

#[tokio::test]
async fn matrix_gateway_uses_authenticated_provenance_and_trusted_time() {
    let gateway = Arc::new(RecordingGateway::default());
    let delivery = Arc::new(RecordingDelivery::default());
    let service = build_agentd_matrix_gateway(MatrixGatewayProviders::new(
        Arc::new(AuthenticatingIdentity {
            identity: identity(),
        }),
        gateway.clone(),
        delivery,
        Arc::new(FixedClock(250)),
    ))
    .expect("gateway composition");

    let receipt = service
        .ingest(event(
            "@operator:matrix.example",
            "@operator:matrix.example",
        ))
        .await
        .expect("ingest");
    assert_eq!(receipt.accepted_at, 250);
    let requests = gateway.requests.lock().expect("requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].observed_at, 250);
    assert_eq!(requests[0].command.class, MatrixCommandClass::Execute);
    assert_eq!(requests[0].command.attachments.len(), 1);
    assert_eq!(requests[0].command.attachments[0].size_bytes, 23);
    assert_eq!(requests[0].command.attachments[0].content_sha256.len(), 64);
}

#[tokio::test]
async fn forged_sender_is_rejected_before_gateway_mutation() {
    let gateway = Arc::new(RecordingGateway::default());
    let delivery = Arc::new(RecordingDelivery::default());
    let service = build_agentd_matrix_gateway(MatrixGatewayProviders::new(
        Arc::new(AuthenticatingIdentity {
            identity: identity(),
        }),
        gateway.clone(),
        delivery,
        Arc::new(FixedClock(250)),
    ))
    .expect("gateway composition");
    let error = service
        .ingest(event("@forged:matrix.example", "@operator:matrix.example"))
        .await
        .expect_err("forged sender");
    assert_eq!(
        error,
        MatrixGatewayError::Denied(MatrixGatewayDenialReason::TransportIdentityMismatch)
    );
    assert!(gateway.requests.lock().expect("requests").is_empty());
}

#[test]
fn command_normalization_hashes_attachments_and_rejects_transcript_shaped_input() {
    let normalized = normalize_command(
        "/run status agentd:run/run_01",
        &[MatrixInboundAttachment {
            bytes: b"artifact".to_vec(),
            media_type: "application/octet-stream".to_string(),
        }],
    )
    .expect("normalize");
    assert_eq!(normalized.class, MatrixCommandClass::Status);
    assert_eq!(normalized.arguments, ["agentd:run/run_01"]);
    assert_eq!(normalized.command_sha256.len(), 64);
    assert_eq!(normalized.attachments[0].content_sha256.len(), 64);
    assert_eq!(normalized.attachments[0].size_bytes, 8);
    assert!(normalize_command("runtime transcript follows", &[]).is_err());
}

#[tokio::test]
async fn pending_semantic_summaries_are_delivered_and_acknowledged_in_order() {
    let gateway = Arc::new(RecordingGateway::default());
    let delivery = Arc::new(RecordingDelivery::default());
    let outbox_id = MatrixGatewayOutboxId::new();
    gateway
        .outbox
        .lock()
        .expect("outbox")
        .push(MatrixGatewayOutboxRecord {
            sequence: 1,
            outbox_id: outbox_id.clone(),
            command_id: MatrixCommandId::new(),
            room_id: "!project-a:matrix.example".to_string(),
            event_kind: "execution_summary".to_string(),
            summary: "execution failed: provider_unavailable".to_string(),
            actionable_links: vec!["agentd:run/r_01".to_string()],
            payload_sha256: "d".repeat(64),
            created_at: 240,
            delivered_at: None,
        });
    let service = build_agentd_matrix_gateway(MatrixGatewayProviders::new(
        Arc::new(AuthenticatingIdentity {
            identity: identity(),
        }),
        gateway.clone(),
        delivery.clone(),
        Arc::new(FixedClock(250)),
    ))
    .expect("gateway composition");

    assert_eq!(
        service
            .deliver_pending(None, 10)
            .await
            .expect("deliver pending"),
        1
    );
    assert_eq!(
        delivery.outbox_ids.lock().expect("delivery").as_slice(),
        [outbox_id]
    );
    assert_eq!(
        gateway.outbox.lock().expect("outbox")[0].delivered_at,
        Some(250)
    );
}

#[test]
fn missing_enterprise_provider_fails_gateway_startup_closed() {
    let providers = MatrixGatewayProviders::new(
        Arc::new(AuthenticatingIdentity {
            identity: identity(),
        }),
        Arc::new(RecordingGateway::default()),
        Arc::new(RecordingDelivery::default()),
        Arc::new(FixedClock(250)),
    )
    .without(MatrixGatewayProviderKind::TrustedClock);
    let error = build_agentd_matrix_gateway(providers).expect_err("missing trusted clock");
    assert!(error.to_string().contains("trusted_clock"));
}
