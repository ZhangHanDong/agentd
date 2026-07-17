use agentd_matrix::{
    AgentdBridgeBackend, BridgeError, BridgeRuntime, BridgeState, MatrixBotAgentSummary,
    MatrixBotCommandAcl, MatrixBotCommandAuthReason, MatrixBotCommandBackendEffectPort,
    MatrixBotCommandMutationResult, MatrixBotCommandSnapshot, MatrixBotCommandTier,
    MatrixBotDmRoomResult, MatrixBotDmRoomStatus, MatrixBotGroupSummary, MatrixBridgeTransport,
    MatrixClientBridgeTransport, MatrixClientInvite, MatrixClientPort, MatrixClientRoom,
    MatrixClientSync, MatrixClientTextMessage, MatrixClientTransportConfig, MatrixInboundEvent,
    MatrixOutboundEvent, MatrixPuppetDirectory, MatrixPuppetProvisioningAction,
    MatrixPuppetProvisioningConfig, MatrixPuppetProvisioningPlan, MatrixPuppetRegistrationAuth,
    MatrixPuppetTokenState, MatrixRoomRegistration, MatrixTrustMode,
};
use serde_json::json;

#[derive(Debug, Clone)]
struct FakeMatrixClient {
    calls: Vec<String>,
    logged_in_user_id: String,
    sync: MatrixClientSync,
    sent: Vec<(String, String)>,
    fail_send_body: Option<String>,
    dm_requests: Vec<(String, String)>,
    dm_result: MatrixBotDmRoomResult,
    room_member_statuses: Vec<(String, String, Option<MatrixBotDmRoomStatus>)>,
    created_direct_rooms: Vec<(String, Vec<String>)>,
    next_direct_room_id: Option<String>,
    invited_users: Vec<(String, String)>,
    fail_invite: Option<String>,
}

impl Default for FakeMatrixClient {
    fn default() -> Self {
        Self {
            calls: Vec::new(),
            logged_in_user_id: "@agent-bridge:matrix.test".to_owned(),
            sync: MatrixClientSync::default(),
            sent: Vec::new(),
            fail_send_body: None,
            dm_requests: Vec::new(),
            dm_result: MatrixBotDmRoomResult::missing_room(),
            room_member_statuses: Vec::new(),
            created_direct_rooms: Vec::new(),
            next_direct_room_id: None,
            invited_users: Vec::new(),
            fail_invite: None,
        }
    }
}

impl MatrixClientPort for FakeMatrixClient {
    fn ensure_logged_in(&mut self) -> Result<String, BridgeError> {
        self.calls.push("ensure_logged_in".to_owned());
        Ok(self.logged_in_user_id.clone())
    }

    fn sync_once(&mut self) -> Result<MatrixClientSync, BridgeError> {
        self.calls.push("sync_once".to_owned());
        Ok(self.sync.clone())
    }

    fn join_room(&mut self, room_id: &str) -> Result<(), BridgeError> {
        self.calls.push(format!("join_room:{room_id}"));
        Ok(())
    }

    fn leave_room(&mut self, room_id: &str) -> Result<(), BridgeError> {
        self.calls.push(format!("leave_room:{room_id}"));
        Ok(())
    }

    fn send_text_message(&mut self, room_id: &str, body: &str) -> Result<(), BridgeError> {
        self.calls
            .push(format!("send_text_message:{room_id}:{body}"));
        if self.fail_send_body.as_deref() == Some(body) {
            return Err(BridgeError::transport(format!(
                "fake Matrix send failed in {room_id}"
            )));
        }
        self.sent.push((room_id.to_owned(), body.to_owned()));
        Ok(())
    }

    fn room_member_status(
        &mut self,
        room_id: &str,
        user_mxid: &str,
    ) -> Result<Option<MatrixBotDmRoomStatus>, BridgeError> {
        self.calls
            .push(format!("room_member_status:{room_id}:{user_mxid}"));
        Ok(self
            .room_member_statuses
            .iter()
            .find(|(candidate_room, candidate_user, _)| {
                candidate_room == room_id && candidate_user == user_mxid
            })
            .and_then(|(_, _, status)| *status))
    }

    fn create_direct_room(
        &mut self,
        name: &str,
        invite_mxids: &[String],
    ) -> Result<String, BridgeError> {
        self.calls.push(format!("create_direct_room:{name}"));
        self.created_direct_rooms
            .push((name.to_owned(), invite_mxids.to_vec()));
        self.next_direct_room_id
            .clone()
            .ok_or_else(|| BridgeError::transport("fake direct room id missing"))
    }

    fn invite_user_to_room(&mut self, room_id: &str, user_mxid: &str) -> Result<(), BridgeError> {
        self.calls
            .push(format!("invite_user_to_room:{room_id}:{user_mxid}"));
        self.invited_users
            .push((room_id.to_owned(), user_mxid.to_owned()));
        if let Some(error) = &self.fail_invite {
            return Err(BridgeError::transport(error.clone()));
        }
        Ok(())
    }

    fn ensure_human_dm_room(
        &mut self,
        agent_name: &str,
        human_mxid: &str,
    ) -> Result<MatrixBotDmRoomResult, BridgeError> {
        self.calls
            .push(format!("ensure_human_dm_room:{agent_name}:{human_mxid}"));
        self.dm_requests
            .push((agent_name.to_owned(), human_mxid.to_owned()));
        Ok(self.dm_result.clone())
    }
}

#[derive(Debug, Default)]
struct FakeBackend {
    outbox: Vec<MatrixOutboundEvent>,
    polled_from: Vec<i64>,
    agents: Vec<MatrixBotAgentSummary>,
    bot_agent_lookups: Vec<String>,
    identity_updates: Vec<(String, String)>,
    identity_result: MatrixBotCommandMutationResult,
    group_creates: Vec<(String, Vec<String>)>,
    group_member_updates: Vec<(String, Vec<String>, Vec<String>)>,
    group_deletes: Vec<String>,
}

impl AgentdBridgeBackend for FakeBackend {
    fn register_room(&mut self, _room: MatrixRoomRegistration) -> Result<(), BridgeError> {
        Ok(())
    }

    fn post_inbound(&mut self, _event: MatrixInboundEvent) -> Result<(), BridgeError> {
        Ok(())
    }

    fn poll_outbox(&mut self, from_seq: i64) -> Result<Vec<MatrixOutboundEvent>, BridgeError> {
        self.polled_from.push(from_seq);
        Ok(self.outbox.clone())
    }
}

impl MatrixBotCommandBackendEffectPort for FakeBackend {
    fn lookup_bot_agent(
        &mut self,
        agent_name: &str,
    ) -> Result<Option<MatrixBotAgentSummary>, BridgeError> {
        self.bot_agent_lookups.push(agent_name.to_owned());
        Ok(self
            .agents
            .iter()
            .find(|agent| agent.name == agent_name)
            .cloned())
    }

    fn update_bot_agent_identity(
        &mut self,
        agent_name: &str,
        identity: &str,
    ) -> Result<MatrixBotCommandMutationResult, BridgeError> {
        self.identity_updates
            .push((agent_name.to_owned(), identity.to_owned()));
        Ok(self.identity_result.clone())
    }

    fn create_bot_group(
        &mut self,
        name: &str,
        members: &[String],
    ) -> Result<MatrixBotCommandMutationResult, BridgeError> {
        self.group_creates.push((name.to_owned(), members.to_vec()));
        Ok(MatrixBotCommandMutationResult::ok())
    }

    fn lookup_bot_group(
        &mut self,
        group_name: &str,
    ) -> Result<Option<MatrixBotGroupSummary>, BridgeError> {
        Ok(Some(MatrixBotGroupSummary {
            name: group_name.to_owned(),
            members: Vec::new(),
        }))
    }

    fn update_bot_group_members(
        &mut self,
        group_name: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<MatrixBotCommandMutationResult, BridgeError> {
        self.group_member_updates
            .push((group_name.to_owned(), add.to_vec(), remove.to_vec()));
        Ok(MatrixBotCommandMutationResult::ok())
    }

    fn delete_bot_group(
        &mut self,
        group_name: &str,
    ) -> Result<MatrixBotCommandMutationResult, BridgeError> {
        self.group_deletes.push(group_name.to_owned());
        Ok(MatrixBotCommandMutationResult::ok())
    }
}

fn config(trust_mode: MatrixTrustMode) -> MatrixClientTransportConfig {
    MatrixClientTransportConfig {
        bot_user_id: None,
        agent_user_prefix: "ac_".to_owned(),
        matrix_server_name: None,
        known_agent_names: Vec::new(),
        skip_agent_names: Vec::new(),
        trust_mode,
        trusted_inviter_mxids: vec!["@alex:matrix.test".to_owned()],
        ignored_sender_mxids: vec!["@ignored:matrix.test".to_owned()],
        bot_command_acl: MatrixBotCommandAcl::default(),
    }
}

fn puppet_config() -> MatrixClientTransportConfig {
    MatrixClientTransportConfig {
        matrix_server_name: Some("matrix.test".to_owned()),
        known_agent_names: vec!["codex-worker".to_owned(), "openfab-bridge".to_owned()],
        skip_agent_names: vec!["openfab-bridge".to_owned()],
        ..config(MatrixTrustMode::Audit)
    }
}

fn command_snapshot() -> MatrixBotCommandSnapshot {
    MatrixBotCommandSnapshot {
        agents: vec![MatrixBotAgentSummary {
            name: "codex-worker".to_owned(),
            status: "online".to_owned(),
            role: Some("coding".to_owned()),
            capability: Some("strong".to_owned()),
            runtime: Some("codex".to_owned()),
        }],
        groups: vec![MatrixBotGroupSummary {
            name: "ops".to_owned(),
            members: vec!["codex-worker".to_owned()],
        }],
        runtime_sessions: None,
        bridge_running: true,
    }
}

fn joined_group_room() -> MatrixClientRoom {
    MatrixClientRoom {
        room_id: "!ops:matrix.test".to_owned(),
        group_name: Some("ops".to_owned()),
        agent_name: None,
        trusted: true,
        trust_reason: "managed".to_owned(),
        inviter_mxid: Some("@alex:matrix.test".to_owned()),
        members: vec!["codex-worker".to_owned(), "codex-reviewer".to_owned()],
    }
}

fn joined_direct_room() -> MatrixClientRoom {
    MatrixClientRoom {
        room_id: "!codex-worker:matrix.test".to_owned(),
        group_name: None,
        agent_name: Some("codex-worker".to_owned()),
        trusted: true,
        trust_reason: "managed".to_owned(),
        inviter_mxid: Some("@alex:matrix.test".to_owned()),
        members: vec!["codex-worker".to_owned()],
    }
}

fn invite(room_id: &str, inviter_mxid: &str) -> MatrixClientInvite {
    MatrixClientInvite {
        room_id: room_id.to_owned(),
        group_name: Some("ops".to_owned()),
        agent_name: None,
        inviter_mxid: Some(inviter_mxid.to_owned()),
        members: vec!["codex-worker".to_owned()],
    }
}

fn text_event(
    event_id: &str,
    sender_mxid: &str,
    body: &str,
    mentions: Vec<&str>,
    reply_to: Option<&str>,
) -> MatrixClientTextMessage {
    text_event_with_formatted(event_id, sender_mxid, body, None, mentions, reply_to)
}

fn text_event_with_formatted(
    event_id: &str,
    sender_mxid: &str,
    body: &str,
    formatted_body: Option<&str>,
    mentions: Vec<&str>,
    reply_to: Option<&str>,
) -> MatrixClientTextMessage {
    MatrixClientTextMessage {
        event_id: event_id.to_owned(),
        room_id: "!ops:matrix.test".to_owned(),
        sender_mxid: sender_mxid.to_owned(),
        body: body.to_owned(),
        formatted_body: formatted_body.map(ToOwned::to_owned),
        mentions: mentions.into_iter().map(ToOwned::to_owned).collect(),
        reply_to: reply_to.map(ToOwned::to_owned),
    }
}

fn outbound_event(seq: i64, body: &str) -> MatrixOutboundEvent {
    MatrixOutboundEvent {
        seq,
        room_id: None,
        target: Some("codex-worker".to_owned()),
        body: body.to_owned(),
        message_id: Some(format!("msg-{seq}")),
        source: Some("api".to_owned()),
        payload: json!({
            "messageId": format!("msg-{seq}"),
            "source": "api",
            "target": "codex-worker",
            "summary": body
        }),
    }
}

#[test]
fn matrix_puppet_directory_plans_known_non_skipped_agent_mxids() {
    let directory = MatrixPuppetDirectory::new(
        "matrix.test",
        "ac_",
        [
            "codex-worker",
            "Codex-Worker",
            "codex-reviewer",
            "openfab-bridge",
        ],
        ["OPENFAB-BRIDGE"],
    )
    .expect("puppet directory");

    let accounts = directory.accounts();

    assert_eq!(accounts.len(), 2);
    assert_eq!(accounts[0].agent_name, "codex-worker");
    assert_eq!(accounts[0].localpart, "ac_codex-worker");
    assert_eq!(accounts[0].mxid, "@ac_codex-worker:matrix.test");
    assert_eq!(accounts[1].agent_name, "codex-reviewer");
    assert_eq!(accounts[1].localpart, "ac_codex-reviewer");
    assert_eq!(accounts[1].mxid, "@ac_codex-reviewer:matrix.test");
    assert!(directory.account_for_agent("CODEX-WORKER").is_some());
    assert!(directory.account_for_agent("openfab-bridge").is_none());
}

#[test]
fn matrix_puppet_directory_resolves_only_known_local_puppet_mxids() {
    let directory = MatrixPuppetDirectory::new(
        "matrix.test",
        "ac_",
        ["codex-worker", "openfab-bridge"],
        ["openfab-bridge"],
    )
    .expect("puppet directory");

    assert_eq!(
        directory.agent_name_from_mxid("@ac_codex-worker:matrix.test"),
        Some("codex-worker")
    );
    assert!(directory.is_agent_puppet_mxid("@ac_codex-worker:matrix.test"));
    assert_eq!(
        directory.agent_name_from_mxid("@ac_unknown:matrix.test"),
        None
    );
    assert_eq!(
        directory.agent_name_from_mxid("@ac_openfab-bridge:matrix.test"),
        None
    );
    assert_eq!(
        directory.agent_name_from_mxid("@ac_codex-worker:elsewhere.test"),
        None
    );
    assert_eq!(directory.agent_name_from_mxid("@alex:matrix.test"), None);
}

#[test]
fn matrix_puppet_directory_rejects_invalid_identity_inputs() {
    let cases: [(&str, &str, Vec<&str>, Vec<&str>); 5] = [
        ("", "ac_", vec!["codex-worker"], vec![]),
        ("matrix.test", "", vec!["codex-worker"], vec![]),
        ("matrix.test", "ac_", vec![""], vec![]),
        (
            "matrix.test",
            "ac_",
            vec!["@codex-worker:matrix.test"],
            vec![],
        ),
        ("matrix.test", "ac_", vec!["codex:worker"], vec![]),
    ];

    for (server_name, prefix, agents, skipped) in cases {
        let err = MatrixPuppetDirectory::new(server_name, prefix, agents, skipped)
            .expect_err("invalid puppet config");
        assert!(
            matches!(err, BridgeError::InvalidConfig(_)),
            "invalid identity input should map to InvalidConfig: {err}"
        );
    }
}

#[test]
fn matrix_puppet_provisioning_config_derives_ordered_password_candidates() {
    let config = MatrixPuppetProvisioningConfig {
        password_secret: Some("top-secret".to_owned()),
        legacy_password_template: Some("legacy-{name}-${name}".to_owned()),
        allow_legacy_password: true,
        registration_token: None,
    };

    let candidates = config.password_candidates("codex-worker");

    assert_eq!(
        candidates,
        vec![
            "c4f10b1c3cc3e2185e56f952f860962a7085d2ccd52a930363e2cc53b9567000".to_owned(),
            "legacy-codex-worker-$codex-worker".to_owned(),
        ]
    );

    let deduped = MatrixPuppetProvisioningConfig {
        legacy_password_template: Some(
            "c4f10b1c3cc3e2185e56f952f860962a7085d2ccd52a930363e2cc53b9567000".to_owned(),
        ),
        ..config.clone()
    };
    assert_eq!(
        deduped.password_candidates("codex-worker"),
        vec!["c4f10b1c3cc3e2185e56f952f860962a7085d2ccd52a930363e2cc53b9567000"]
    );

    let disabled = MatrixPuppetProvisioningConfig {
        password_secret: Some(" ".to_owned()),
        legacy_password_template: Some("legacy-{name}".to_owned()),
        allow_legacy_password: false,
        registration_token: None,
    };
    assert!(disabled.password_candidates("codex-worker").is_empty());
}

#[test]
fn matrix_puppet_provisioning_plan_reuses_tokens_and_schedules_missing_accounts() {
    let directory = MatrixPuppetDirectory::new(
        "matrix.test",
        "ac_",
        ["codex-worker", "codex-reviewer", "openfab-bridge"],
        ["openfab-bridge"],
    )
    .expect("puppet directory");
    let token_state = MatrixPuppetTokenState::from_agent_tokens([
        ("Codex-Worker", "worker-token"),
        ("old-agent", "stale-token"),
    ]);
    let config = MatrixPuppetProvisioningConfig {
        password_secret: Some("top-secret".to_owned()),
        ..MatrixPuppetProvisioningConfig::default()
    };

    let plan = MatrixPuppetProvisioningPlan::from_directory(&directory, &config, &token_state);

    assert_eq!(plan.stale_token_names(), &["old-agent".to_owned()]);
    assert_eq!(plan.actions().len(), 2);

    match &plan.actions()[0] {
        MatrixPuppetProvisioningAction::ReuseToken {
            agent_name,
            localpart,
            mxid,
            token_name,
        } => {
            assert_eq!(agent_name, "codex-worker");
            assert_eq!(localpart, "ac_codex-worker");
            assert_eq!(mxid, "@ac_codex-worker:matrix.test");
            assert_eq!(token_name, "Codex-Worker");
        }
        other => panic!("expected token reuse action, got {other:?}"),
    }

    match &plan.actions()[1] {
        MatrixPuppetProvisioningAction::LoginOrRegister {
            agent_name,
            localpart,
            mxid,
            password_candidates,
        } => {
            assert_eq!(agent_name, "codex-reviewer");
            assert_eq!(localpart, "ac_codex-reviewer");
            assert_eq!(mxid, "@ac_codex-reviewer:matrix.test");
            assert_eq!(password_candidates.len(), 1);
        }
        other => panic!("expected login/register action, got {other:?}"),
    }
}

#[test]
fn matrix_puppet_provisioning_plan_reports_missing_password_candidates() {
    let directory =
        MatrixPuppetDirectory::new("matrix.test", "ac_", ["codex-worker"], Vec::<&str>::new())
            .expect("puppet directory");
    let token_state = MatrixPuppetTokenState::default();
    let config = MatrixPuppetProvisioningConfig::default();

    let plan = MatrixPuppetProvisioningPlan::from_directory(&directory, &config, &token_state);

    assert!(plan.stale_token_names().is_empty());
    assert_eq!(plan.actions().len(), 1);
    match &plan.actions()[0] {
        MatrixPuppetProvisioningAction::MissingPassword {
            agent_name,
            localpart,
            mxid,
        } => {
            assert_eq!(agent_name, "codex-worker");
            assert_eq!(localpart, "ac_codex-worker");
            assert_eq!(mxid, "@ac_codex-worker:matrix.test");
        }
        other => panic!("expected missing-password action, got {other:?}"),
    }
}

#[test]
fn matrix_puppet_registration_auth_plan_selects_token_dummy_or_error() {
    let token_config = MatrixPuppetProvisioningConfig {
        registration_token: Some("reg-token".to_owned()),
        ..MatrixPuppetProvisioningConfig::default()
    };
    assert_eq!(
        token_config
            .registration_auth("uia-session", false)
            .expect("registration token auth"),
        MatrixPuppetRegistrationAuth::RegistrationToken {
            token: "reg-token".to_owned(),
            session: "uia-session".to_owned(),
        }
    );

    let dummy_config = MatrixPuppetProvisioningConfig::default();
    assert_eq!(
        dummy_config
            .registration_auth("dummy-session", true)
            .expect("dummy auth"),
        MatrixPuppetRegistrationAuth::Dummy {
            session: "dummy-session".to_owned(),
        }
    );

    let err = dummy_config
        .registration_auth("no-flow-session", false)
        .expect_err("no usable registration flow");
    assert!(
        matches!(err, BridgeError::InvalidConfig(_)),
        "no usable Matrix registration flow should be InvalidConfig: {err}"
    );
}

#[test]
fn matrix_client_transport_logs_in_joins_trusted_invites_and_registers_rooms() {
    let fake = FakeMatrixClient {
        sync: MatrixClientSync {
            invites: vec![invite("!trusted:matrix.test", "@alex:matrix.test")],
            joined_rooms: vec![joined_group_room()],
            text_events: Vec::new(),
        },
        ..FakeMatrixClient::default()
    };
    let mut transport = MatrixClientBridgeTransport::new(fake, config(MatrixTrustMode::Audit));

    let registrations = transport.room_registrations().expect("room registrations");

    assert_eq!(
        transport.client().calls,
        vec![
            "ensure_logged_in",
            "sync_once",
            "join_room:!trusted:matrix.test"
        ]
    );
    assert_eq!(registrations.len(), 2);
    assert_eq!(registrations[0].room_id, "!trusted:matrix.test");
    assert!(registrations[0].trusted);
    assert_eq!(registrations[0].trust_reason, "trusted_inviter");
    assert_eq!(
        registrations[0].inviter_mxid.as_deref(),
        Some("@alex:matrix.test")
    );
    assert_eq!(registrations[1].room_id, "!ops:matrix.test");
    assert_eq!(registrations[1].group_name.as_deref(), Some("ops"));
}

#[test]
fn matrix_client_transport_enforces_untrusted_invite_policy() {
    let untrusted = invite("!untrusted:matrix.test", "@mallory:matrix.test");
    let audit_fake = FakeMatrixClient {
        sync: MatrixClientSync {
            invites: vec![untrusted.clone()],
            joined_rooms: Vec::new(),
            text_events: Vec::new(),
        },
        ..FakeMatrixClient::default()
    };
    let mut audit_transport =
        MatrixClientBridgeTransport::new(audit_fake, config(MatrixTrustMode::Audit));

    let audit_regs = audit_transport
        .room_registrations()
        .expect("audit registrations");

    assert_eq!(
        audit_transport.client().calls,
        vec![
            "ensure_logged_in",
            "sync_once",
            "join_room:!untrusted:matrix.test"
        ]
    );
    assert_eq!(audit_regs.len(), 1);
    assert!(!audit_regs[0].trusted);
    assert_eq!(audit_regs[0].trust_reason, "untrusted_inviter");

    let enforce_fake = FakeMatrixClient {
        sync: MatrixClientSync {
            invites: vec![untrusted],
            joined_rooms: Vec::new(),
            text_events: Vec::new(),
        },
        ..FakeMatrixClient::default()
    };
    let mut enforce_transport =
        MatrixClientBridgeTransport::new(enforce_fake, config(MatrixTrustMode::Enforce));

    let enforce_regs = enforce_transport
        .room_registrations()
        .expect("enforce registrations");

    assert_eq!(
        enforce_transport.client().calls,
        vec![
            "ensure_logged_in",
            "sync_once",
            "leave_room:!untrusted:matrix.test"
        ]
    );
    assert!(enforce_regs.is_empty());
}

#[test]
fn matrix_client_transport_normalizes_inbound_text_and_suppresses_loops() {
    let fake = FakeMatrixClient {
        sync: MatrixClientSync {
            invites: Vec::new(),
            joined_rooms: vec![joined_group_room()],
            text_events: vec![
                text_event(
                    "$human",
                    "@alex:matrix.test",
                    "please review",
                    vec!["codex-worker"],
                    Some("msg-parent"),
                ),
                text_event(
                    "$bot",
                    "@agent-bridge:matrix.test",
                    "bot loop",
                    vec![],
                    None,
                ),
                text_event(
                    "$agent",
                    "@ac_codex:matrix.test",
                    "agent loop",
                    vec![],
                    None,
                ),
                text_event("$ignored", "@ignored:matrix.test", "ignored", vec![], None),
                text_event(
                    "$agentignore",
                    "@alex:matrix.test",
                    "[AGENTIGNORE] echoed",
                    vec![],
                    None,
                ),
            ],
        },
        ..FakeMatrixClient::default()
    };
    let mut transport = MatrixClientBridgeTransport::new(fake, config(MatrixTrustMode::Audit));

    let inbound = transport.inbound_events().expect("inbound events");

    assert_eq!(inbound.len(), 1);
    assert_eq!(inbound[0].event_id, "$human");
    assert_eq!(inbound[0].room_id, "!ops:matrix.test");
    assert_eq!(inbound[0].sender_mxid, "@alex:matrix.test");
    assert_eq!(inbound[0].body, "please review");
    assert_eq!(inbound[0].mentions, vec!["codex-worker"]);
    assert_eq!(inbound[0].reply_to.as_deref(), Some("msg-parent"));
}

#[test]
fn matrix_client_transport_separates_bot_commands_from_inbound_events() {
    let mut cfg = config(MatrixTrustMode::Audit);
    cfg.bot_command_acl.operator_mxids = vec!["@alex:matrix.test".to_owned()];
    let fake = FakeMatrixClient {
        sync: MatrixClientSync {
            invites: Vec::new(),
            joined_rooms: vec![joined_group_room()],
            text_events: vec![
                text_event("$cmd", "@alex:matrix.test", "!status", Vec::new(), None),
                text_event(
                    "$normal",
                    "@alex:matrix.test",
                    "please review",
                    vec!["codex-worker"],
                    Some("msg-parent"),
                ),
            ],
        },
        ..FakeMatrixClient::default()
    };
    let mut transport = MatrixClientBridgeTransport::new(fake, cfg);

    let inbound = transport.inbound_events().expect("inbound events");
    let commands = transport
        .bot_command_plans()
        .expect("bot command plans after sync");

    assert_eq!(inbound.len(), 1);
    assert_eq!(inbound[0].event_id, "$normal");
    assert_eq!(inbound[0].body, "please review");
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].event_id.as_deref(), Some("$cmd"));
    assert_eq!(commands[0].room_id.as_deref(), Some("!ops:matrix.test"));
    assert_eq!(commands[0].sender_mxid, "@alex:matrix.test");
    assert_eq!(commands[0].command, "!status");
    assert_eq!(commands[0].tier, MatrixBotCommandTier::OperatorRead);
    assert!(commands[0].authorization.allowed);
    assert_eq!(
        commands[0].authorization.reason,
        MatrixBotCommandAuthReason::Operator
    );
    assert_eq!(commands[0].group_name.as_deref(), Some("ops"));
    assert_eq!(commands[0].target_agent, None);
}

#[test]
fn matrix_client_transport_sends_bot_command_replies_without_forwarding_inbound() {
    let mut cfg = config(MatrixTrustMode::Audit);
    cfg.bot_command_acl.operator_mxids = vec!["@alex:matrix.test".to_owned()];
    let fake = FakeMatrixClient {
        sync: MatrixClientSync {
            invites: Vec::new(),
            joined_rooms: vec![joined_group_room()],
            text_events: vec![
                text_event("$cmd", "@alex:matrix.test", "!status", Vec::new(), None),
                text_event(
                    "$normal",
                    "@alex:matrix.test",
                    "please review",
                    vec!["codex-worker"],
                    Some("msg-parent"),
                ),
            ],
        },
        ..FakeMatrixClient::default()
    };
    let mut transport = MatrixClientBridgeTransport::new(fake, cfg);

    let replies = transport
        .execute_bot_command_replies(&command_snapshot())
        .expect("bot command replies");
    let commands = transport.bot_command_plans().expect("bot command plans");
    let inbound = transport.inbound_events().expect("inbound events");

    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].room_id, "!ops:matrix.test");
    assert!(replies[0].body.contains("=== System Status ==="));
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].command, "!status");
    assert_eq!(inbound.len(), 1);
    assert_eq!(inbound[0].event_id, "$normal");
    assert_eq!(
        transport.client().sent,
        vec![("!ops:matrix.test".to_owned(), replies[0].body.clone())]
    );
}

#[test]
fn matrix_client_transport_executes_management_commands_through_effect_ports() {
    let mut cfg = puppet_config();
    cfg.bot_command_acl.operator_mxids = vec!["@alex:matrix.test".to_owned()];
    let mut identity_command = text_event(
        "$identity",
        "@alex:matrix.test",
        "!identity Be concise",
        Vec::new(),
        None,
    );
    identity_command.room_id = "!codex-worker:matrix.test".to_owned();
    let fake = FakeMatrixClient {
        sync: MatrixClientSync {
            invites: Vec::new(),
            joined_rooms: vec![joined_group_room(), joined_direct_room()],
            text_events: vec![
                text_event(
                    "$dm",
                    "@alex:matrix.test",
                    "!dm codex-worker",
                    Vec::new(),
                    None,
                ),
                identity_command,
                text_event(
                    "$normal",
                    "@alex:matrix.test",
                    "please review",
                    vec!["codex-worker"],
                    Some("msg-parent"),
                ),
            ],
        },
        dm_result: MatrixBotDmRoomResult {
            room_id: Some("!dm-codex-worker:matrix.test".to_owned()),
            human_status: MatrixBotDmRoomStatus::Invited,
            invite_error: None,
        },
        ..FakeMatrixClient::default()
    };
    let mut backend = FakeBackend {
        agents: command_snapshot().agents,
        identity_result: MatrixBotCommandMutationResult::ok(),
        ..FakeBackend::default()
    };
    let mut transport = MatrixClientBridgeTransport::new(fake, cfg);

    let replies = transport
        .execute_bot_command_replies_with_effects(&command_snapshot(), &mut backend)
        .expect("management command replies");
    let inbound = transport.inbound_events().expect("inbound events");

    assert_eq!(
        transport.client().invited_users,
        [(
            "!codex-worker:matrix.test".to_owned(),
            "@alex:matrix.test".to_owned(),
        )]
    );
    assert_eq!(
        backend.identity_updates,
        [("codex-worker".to_owned(), "Be concise".to_owned())]
    );
    assert_eq!(replies.len(), 2);
    assert_eq!(transport.client().sent.len(), 2);
    assert!(transport.client().sent[0].1.contains("DM room ready"));
    assert!(
        transport.client().sent[1]
            .1
            .contains("Identity set for codex-worker")
    );
    assert_eq!(inbound.len(), 1);
    assert_eq!(inbound[0].event_id, "$normal");
}

#[test]
fn matrix_client_transport_dm_lifecycle_reuses_joined_direct_room() {
    let mut cfg = puppet_config();
    cfg.bot_command_acl.operator_mxids = vec!["@alex:matrix.test".to_owned()];
    let fake = FakeMatrixClient {
        sync: MatrixClientSync {
            invites: Vec::new(),
            joined_rooms: vec![joined_direct_room()],
            text_events: vec![text_event(
                "$dm",
                "@alex:matrix.test",
                "!dm codex-worker",
                Vec::new(),
                None,
            )],
        },
        room_member_statuses: vec![(
            "!codex-worker:matrix.test".to_owned(),
            "@alex:matrix.test".to_owned(),
            Some(MatrixBotDmRoomStatus::Joined),
        )],
        ..FakeMatrixClient::default()
    };
    let mut backend = FakeBackend {
        agents: command_snapshot().agents,
        ..FakeBackend::default()
    };
    let mut transport = MatrixClientBridgeTransport::new(fake, cfg);

    let replies = transport
        .execute_bot_command_replies_with_effects(&command_snapshot(), &mut backend)
        .expect("management command replies");

    assert_eq!(replies.len(), 1);
    assert!(replies[0].body.contains("already in the DM room"));
    assert!(transport.client().created_direct_rooms.is_empty());
    assert!(transport.client().invited_users.is_empty());
}

#[test]
fn matrix_client_transport_dm_lifecycle_creates_missing_direct_room() {
    let mut cfg = puppet_config();
    cfg.bot_command_acl.operator_mxids = vec!["@alex:matrix.test".to_owned()];
    let fake = FakeMatrixClient {
        sync: MatrixClientSync {
            invites: Vec::new(),
            joined_rooms: Vec::new(),
            text_events: vec![text_event(
                "$dm",
                "@alex:matrix.test",
                "!dm codex-worker",
                Vec::new(),
                None,
            )],
        },
        next_direct_room_id: Some("!dm-created:matrix.test".to_owned()),
        ..FakeMatrixClient::default()
    };
    let mut backend = FakeBackend {
        agents: command_snapshot().agents,
        ..FakeBackend::default()
    };
    let mut transport = MatrixClientBridgeTransport::new(fake, cfg);

    let replies = transport
        .execute_bot_command_replies_with_effects(&command_snapshot(), &mut backend)
        .expect("management command replies");

    assert_eq!(
        transport.client().created_direct_rooms,
        [(
            "DM: codex-worker".to_owned(),
            vec![
                "@alex:matrix.test".to_owned(),
                "@ac_codex-worker:matrix.test".to_owned(),
            ],
        )]
    );
    assert_eq!(replies.len(), 1);
    assert!(replies[0].body.contains("DM room ready"));
    assert!(replies[0].body.contains("!dm-created:matrix.test"));
}

#[test]
fn matrix_client_transport_dm_lifecycle_reports_invite_failure_for_existing_room() {
    let mut cfg = puppet_config();
    cfg.bot_command_acl.operator_mxids = vec!["@alex:matrix.test".to_owned()];
    let fake = FakeMatrixClient {
        sync: MatrixClientSync {
            invites: Vec::new(),
            joined_rooms: vec![joined_direct_room()],
            text_events: vec![text_event(
                "$dm",
                "@alex:matrix.test",
                "!dm codex-worker",
                Vec::new(),
                None,
            )],
        },
        room_member_statuses: vec![(
            "!codex-worker:matrix.test".to_owned(),
            "@alex:matrix.test".to_owned(),
            None,
        )],
        fail_invite: Some("M_FORBIDDEN: no invite permission".to_owned()),
        ..FakeMatrixClient::default()
    };
    let mut backend = FakeBackend {
        agents: command_snapshot().agents,
        ..FakeBackend::default()
    };
    let mut transport = MatrixClientBridgeTransport::new(fake, cfg);

    let replies = transport
        .execute_bot_command_replies_with_effects(&command_snapshot(), &mut backend)
        .expect("management command replies");

    assert_eq!(replies.len(), 1);
    assert!(replies[0].body.contains("DM room exists but invite failed"));
    assert!(
        replies[0]
            .body
            .contains("M_FORBIDDEN: no invite permission")
    );
    assert!(transport.client().created_direct_rooms.is_empty());
    assert_eq!(
        transport.client().invited_users,
        [(
            "!codex-worker:matrix.test".to_owned(),
            "@alex:matrix.test".to_owned(),
        )]
    );
}

#[test]
fn matrix_client_transport_joingroup_invites_sender_to_trusted_group_room() {
    let mut cfg = puppet_config();
    cfg.bot_command_acl.operator_mxids = vec!["@alex:matrix.test".to_owned()];
    let mut command = text_event(
        "$joingroup",
        "@alex:matrix.test",
        "!joingroup ops",
        Vec::new(),
        None,
    );
    command.room_id = "!bot:matrix.test".to_owned();
    let fake = FakeMatrixClient {
        sync: MatrixClientSync {
            invites: Vec::new(),
            joined_rooms: vec![joined_group_room()],
            text_events: vec![command],
        },
        room_member_statuses: vec![(
            "!ops:matrix.test".to_owned(),
            "@alex:matrix.test".to_owned(),
            None,
        )],
        ..FakeMatrixClient::default()
    };
    let mut backend = FakeBackend::default();
    let mut transport = MatrixClientBridgeTransport::new(fake, cfg);

    let replies = transport
        .execute_bot_command_replies_with_effects(&command_snapshot(), &mut backend)
        .expect("joingroup replies");

    assert_eq!(
        backend.group_member_updates,
        [(
            "ops".to_owned(),
            vec!["alex".to_owned()],
            Vec::<String>::new()
        )]
    );
    assert_eq!(
        transport.client().invited_users,
        [(
            "!ops:matrix.test".to_owned(),
            "@alex:matrix.test".to_owned()
        )]
    );
    assert_eq!(replies.len(), 1);
    assert_eq!(transport.client().sent.len(), 1);
    assert_eq!(transport.client().sent[0].0, "!bot:matrix.test");
    assert!(
        transport.client().sent[0]
            .1
            .contains("Added you (alex) to group \"ops\"")
    );
}

#[test]
fn matrix_client_transport_joingroup_reports_missing_group_room_mapping() {
    let mut cfg = puppet_config();
    cfg.bot_command_acl.operator_mxids = vec!["@alex:matrix.test".to_owned()];
    let mut command = text_event(
        "$joingroup",
        "@alex:matrix.test",
        "!joingroup ops",
        Vec::new(),
        None,
    );
    command.room_id = "!bot:matrix.test".to_owned();
    let fake = FakeMatrixClient {
        sync: MatrixClientSync {
            invites: Vec::new(),
            joined_rooms: Vec::new(),
            text_events: vec![command],
        },
        ..FakeMatrixClient::default()
    };
    let mut backend = FakeBackend::default();
    let mut transport = MatrixClientBridgeTransport::new(fake, cfg);

    let replies = transport
        .execute_bot_command_replies_with_effects(&command_snapshot(), &mut backend)
        .expect("joingroup replies");

    assert_eq!(
        backend.group_member_updates,
        [(
            "ops".to_owned(),
            vec!["alex".to_owned()],
            Vec::<String>::new()
        )]
    );
    assert!(transport.client().invited_users.is_empty());
    assert_eq!(replies.len(), 1);
    assert!(
        replies[0]
            .body
            .contains("no trusted Matrix group room is mapped")
    );
}

#[test]
fn matrix_client_transport_classifies_formatted_mention_commands() {
    let mut cfg = config(MatrixTrustMode::Audit);
    cfg.bot_command_acl.operator_mxids = vec!["@alex:matrix.test".to_owned()];
    let fake = FakeMatrixClient {
        sync: MatrixClientSync {
            invites: Vec::new(),
            joined_rooms: vec![joined_group_room()],
            text_events: vec![text_event_with_formatted(
                "$mention-cmd",
                "@alex:matrix.test",
                "Agent Bridge: !dm codex-worker",
                Some(
                    r#"<a href="https://matrix.to/#/@agent-bridge:matrix.test">Agent Bridge</a>: !dm codex-worker"#,
                ),
                Vec::new(),
                None,
            )],
        },
        ..FakeMatrixClient::default()
    };
    let mut transport = MatrixClientBridgeTransport::new(fake, cfg);

    let inbound = transport.inbound_events().expect("inbound events");
    let commands = transport.bot_command_plans().expect("bot command plans");

    assert!(inbound.is_empty());
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].command, "!dm");
    assert_eq!(commands[0].args, ["codex-worker"]);
    assert_eq!(commands[0].tier, MatrixBotCommandTier::OperatorManagement);
}

#[test]
fn matrix_client_transport_suppresses_loop_commands_before_planning() {
    let mut cfg = puppet_config();
    cfg.bot_command_acl.operator_mxids = vec!["@operator:matrix.test".to_owned()];
    let fake = FakeMatrixClient {
        sync: MatrixClientSync {
            invites: Vec::new(),
            joined_rooms: vec![joined_group_room()],
            text_events: vec![
                text_event(
                    "$bot-cmd",
                    "@agent-bridge:matrix.test",
                    "!status",
                    Vec::new(),
                    None,
                ),
                text_event(
                    "$puppet-cmd",
                    "@ac_codex-worker:matrix.test",
                    "!status",
                    Vec::new(),
                    None,
                ),
                text_event(
                    "$ignored-cmd",
                    "@ignored:matrix.test",
                    "!status",
                    Vec::new(),
                    None,
                ),
                text_event(
                    "$guest-cmd",
                    "@guest:matrix.test",
                    "!status",
                    Vec::new(),
                    None,
                ),
            ],
        },
        ..FakeMatrixClient::default()
    };
    let mut transport = MatrixClientBridgeTransport::new(fake, cfg);

    let inbound = transport.inbound_events().expect("inbound events");
    let commands = transport.bot_command_plans().expect("bot command plans");

    assert!(inbound.is_empty());
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].event_id.as_deref(), Some("$guest-cmd"));
    assert_eq!(commands[0].command, "!status");
    assert!(!commands[0].authorization.allowed);
    assert_eq!(
        commands[0].authorization.reason,
        MatrixBotCommandAuthReason::OperatorRequired
    );
}

#[test]
fn matrix_client_transport_suppresses_known_puppets_not_unknown_prefix_users() {
    let fake = FakeMatrixClient {
        sync: MatrixClientSync {
            invites: Vec::new(),
            joined_rooms: vec![joined_group_room()],
            text_events: vec![
                text_event(
                    "$bot",
                    "@agent-bridge:matrix.test",
                    "bot echo",
                    Vec::new(),
                    None,
                ),
                text_event(
                    "$known-puppet",
                    "@ac_codex-worker:matrix.test",
                    "worker echo",
                    Vec::new(),
                    None,
                ),
                text_event(
                    "$unknown-prefix",
                    "@ac_unknown:matrix.test",
                    "not a known puppet",
                    Vec::new(),
                    None,
                ),
                text_event(
                    "$skipped-service",
                    "@ac_openfab-bridge:matrix.test",
                    "skipped service user remains visible",
                    Vec::new(),
                    None,
                ),
                text_event(
                    "$ignored-tag",
                    "@alex:matrix.test",
                    "[AGENTIGNORE] hidden",
                    Vec::new(),
                    None,
                ),
            ],
        },
        ..FakeMatrixClient::default()
    };
    let mut transport = MatrixClientBridgeTransport::new(fake, puppet_config());

    let inbound = transport.inbound_events().expect("inbound events");
    let event_ids = inbound
        .iter()
        .map(|event| event.event_id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(event_ids, vec!["$unknown-prefix", "$skipped-service"]);
    assert_eq!(inbound[0].sender_mxid, "@ac_unknown:matrix.test");
    assert_eq!(inbound[1].sender_mxid, "@ac_openfab-bridge:matrix.test");
}

#[test]
fn matrix_client_transport_sends_outbound_text_to_resolved_target_room() {
    let fake = FakeMatrixClient {
        sync: MatrixClientSync {
            invites: Vec::new(),
            joined_rooms: vec![joined_direct_room()],
            text_events: Vec::new(),
        },
        ..FakeMatrixClient::default()
    };
    let mut transport = MatrixClientBridgeTransport::new(fake, config(MatrixTrustMode::Audit));

    transport
        .send_outbound(outbound_event(7, "reply from codex"))
        .expect("send outbound");

    assert_eq!(
        transport.client().sent,
        vec![(
            "!codex-worker:matrix.test".to_owned(),
            "reply from codex".to_owned()
        )]
    );
    assert_eq!(
        transport.client().calls,
        vec![
            "ensure_logged_in",
            "sync_once",
            "send_text_message:!codex-worker:matrix.test:reply from codex"
        ]
    );
}

#[test]
fn matrix_client_transport_send_failure_preserves_runtime_retry_cursor() {
    let backend = FakeBackend {
        outbox: vec![outbound_event(1, "first"), outbound_event(2, "second")],
        ..FakeBackend::default()
    };
    let fake = FakeMatrixClient {
        sync: MatrixClientSync {
            invites: Vec::new(),
            joined_rooms: vec![joined_direct_room()],
            text_events: Vec::new(),
        },
        fail_send_body: Some("second".to_owned()),
        ..FakeMatrixClient::default()
    };
    let transport = MatrixClientBridgeTransport::new(fake, config(MatrixTrustMode::Audit));
    let mut runtime = BridgeRuntime::new(backend, transport, BridgeState::default());

    let err = runtime.run_once().expect_err("second send fails");

    let err_text = err.to_string();
    assert!(
        err_text.contains("!codex-worker:matrix.test"),
        "error should name room id: {err_text}"
    );
    assert_eq!(
        runtime.transport().client().sent,
        vec![("!codex-worker:matrix.test".to_owned(), "first".to_owned())]
    );
    assert_eq!(runtime.state().next_from_seq(), 1);
}
