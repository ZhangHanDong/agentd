use agentd_matrix::{
    MatrixBotAgentSummary, MatrixBotCommandAcl, MatrixBotCommandAuthReason,
    MatrixBotCommandBackendEffectPort, MatrixBotCommandContext, MatrixBotCommandMutationResult,
    MatrixBotCommandPlan, MatrixBotCommandRoomEffectPort, MatrixBotCommandSideEffect,
    MatrixBotCommandSnapshot, MatrixBotCommandTier, MatrixBotDmRoomResult, MatrixBotDmRoomStatus,
    MatrixBotGroupRoomResult, MatrixBotGroupSummary, execute_matrix_bot_command,
    execute_matrix_bot_command_with_effects, plan_matrix_bot_command,
};

fn context() -> MatrixBotCommandContext {
    MatrixBotCommandContext {
        group_name: Some("ops".to_owned()),
        target_agent: Some("codex-worker".to_owned()),
    }
}

fn command(plan: MatrixBotCommandPlan) -> agentd_matrix::MatrixBotCommand {
    match plan {
        MatrixBotCommandPlan::Command(command) => command,
        MatrixBotCommandPlan::NonCommand(fallback) => {
            panic!("expected command plan, got fallback {fallback:?}")
        }
    }
}

fn fallback(plan: MatrixBotCommandPlan) -> agentd_matrix::MatrixBotNonCommandPlan {
    match plan {
        MatrixBotCommandPlan::NonCommand(fallback) => fallback,
        MatrixBotCommandPlan::Command(command) => {
            panic!("expected fallback plan, got command {command:?}")
        }
    }
}

fn command_in_room(
    sender: &str,
    body: &str,
    acl: &MatrixBotCommandAcl,
) -> agentd_matrix::MatrixBotCommand {
    command_in_room_with_context(sender, body, MatrixBotCommandContext::default(), acl)
}

fn command_in_room_with_context(
    sender: &str,
    body: &str,
    context: MatrixBotCommandContext,
    acl: &MatrixBotCommandAcl,
) -> agentd_matrix::MatrixBotCommand {
    let mut command = command(plan_matrix_bot_command(sender, body, None, context, acl));
    command.room_id = Some("!bot:matrix.test".to_owned());
    command.event_id = Some(format!(
        "${}",
        body.trim_start_matches('!').replace(' ', "-")
    ));
    command
}

#[derive(Debug, Default)]
struct FakeManagementBackend {
    agents: Vec<MatrixBotAgentSummary>,
    groups: Vec<MatrixBotGroupSummary>,
    lookups: Vec<String>,
    group_lookups: Vec<String>,
    identity_updates: Vec<(String, String)>,
    group_creates: Vec<(String, Vec<String>)>,
    group_member_updates: Vec<(String, Vec<String>, Vec<String>)>,
    group_deletes: Vec<String>,
    identity_result: MatrixBotCommandMutationResult,
    group_result: MatrixBotCommandMutationResult,
}

impl MatrixBotCommandBackendEffectPort for FakeManagementBackend {
    fn lookup_bot_agent(
        &mut self,
        agent_name: &str,
    ) -> Result<Option<MatrixBotAgentSummary>, agentd_matrix::BridgeError> {
        self.lookups.push(agent_name.to_owned());
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
    ) -> Result<MatrixBotCommandMutationResult, agentd_matrix::BridgeError> {
        self.identity_updates
            .push((agent_name.to_owned(), identity.to_owned()));
        Ok(self.identity_result.clone())
    }

    fn create_bot_group(
        &mut self,
        name: &str,
        members: &[String],
    ) -> Result<MatrixBotCommandMutationResult, agentd_matrix::BridgeError> {
        self.group_creates.push((name.to_owned(), members.to_vec()));
        Ok(self.group_result.clone())
    }

    fn lookup_bot_group(
        &mut self,
        group_name: &str,
    ) -> Result<Option<MatrixBotGroupSummary>, agentd_matrix::BridgeError> {
        self.group_lookups.push(group_name.to_owned());
        Ok(self
            .groups
            .iter()
            .find(|group| group.name == group_name)
            .cloned())
    }

    fn update_bot_group_members(
        &mut self,
        group_name: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<MatrixBotCommandMutationResult, agentd_matrix::BridgeError> {
        self.group_member_updates
            .push((group_name.to_owned(), add.to_vec(), remove.to_vec()));
        Ok(self.group_result.clone())
    }

    fn delete_bot_group(
        &mut self,
        group_name: &str,
    ) -> Result<MatrixBotCommandMutationResult, agentd_matrix::BridgeError> {
        self.group_deletes.push(group_name.to_owned());
        Ok(self.group_result.clone())
    }
}

#[derive(Debug, Default)]
struct FakeRoomEffects {
    dm_requests: Vec<(String, String)>,
    dm_result: MatrixBotDmRoomResult,
    group_room_requests: Vec<(String, String)>,
    group_room_result: MatrixBotGroupRoomResult,
}

impl MatrixBotCommandRoomEffectPort for FakeRoomEffects {
    fn ensure_human_dm_room(
        &mut self,
        agent_name: &str,
        human_localpart: &str,
    ) -> Result<MatrixBotDmRoomResult, agentd_matrix::BridgeError> {
        self.dm_requests
            .push((agent_name.to_owned(), human_localpart.to_owned()));
        Ok(self.dm_result.clone())
    }

    fn ensure_human_group_room(
        &mut self,
        group_name: &str,
        human_mxid: &str,
    ) -> Result<MatrixBotGroupRoomResult, agentd_matrix::BridgeError> {
        self.group_room_requests
            .push((group_name.to_owned(), human_mxid.to_owned()));
        Ok(self.group_room_result.clone())
    }
}

fn snapshot() -> MatrixBotCommandSnapshot {
    MatrixBotCommandSnapshot {
        agents: vec![
            MatrixBotAgentSummary {
                name: "codex-worker".to_owned(),
                status: "online".to_owned(),
                role: Some("coding".to_owned()),
                capability: Some("strong".to_owned()),
                runtime: Some("codex".to_owned()),
            },
            MatrixBotAgentSummary {
                name: "codex-reviewer".to_owned(),
                status: "offline".to_owned(),
                role: Some("review".to_owned()),
                capability: Some("medium".to_owned()),
                runtime: Some("codex".to_owned()),
            },
        ],
        groups: vec![MatrixBotGroupSummary {
            name: "ops".to_owned(),
            members: vec!["codex-worker".to_owned(), "codex-reviewer".to_owned()],
        }],
        tmux_sessions: None,
        bridge_running: true,
    }
}

#[test]
fn matrix_bot_command_parser_recognizes_bang_commands_and_preserves_args() {
    let acl = MatrixBotCommandAcl::default();

    let command = command(plan_matrix_bot_command(
        "@alex:matrix.test",
        "  !IDENTITY codex-worker Be concise  ",
        None,
        context(),
        &acl,
    ));

    assert_eq!(command.command, "!identity");
    assert_eq!(command.args, ["codex-worker", "Be", "concise"]);
    assert_eq!(command.tier, MatrixBotCommandTier::OperatorManagement);
    assert!(command.authorization.allowed);
    assert_eq!(
        command.authorization.reason,
        MatrixBotCommandAuthReason::NoAcl
    );
    assert_eq!(command.sender_human_localpart, "alex");
    assert_eq!(command.group_name.as_deref(), Some("ops"));
    assert_eq!(command.target_agent.as_deref(), Some("codex-worker"));
}

#[test]
fn matrix_bot_command_parser_strips_matrix_mention_prefix() {
    let formatted_body =
        r#"<a href="https://matrix.to/#/@agent-bridge:matrix.test">Agent Bridge</a>: !status"#;

    let command = command(plan_matrix_bot_command(
        "@alex:matrix.test",
        "Agent Bridge: !status",
        Some(formatted_body),
        MatrixBotCommandContext::default(),
        &MatrixBotCommandAcl::default(),
    ));

    assert_eq!(command.command, "!status");
    assert!(command.args.is_empty());
    assert_eq!(command.tier, MatrixBotCommandTier::OperatorRead);
}

#[test]
fn matrix_bot_command_acl_matches_agent_chat_tiers() {
    let acl = MatrixBotCommandAcl {
        operator_mxids: vec!["@operator:matrix.test".to_owned()],
        admin_mxids: vec!["@admin:matrix.test".to_owned()],
    };

    let public_help = command(plan_matrix_bot_command(
        "@guest:matrix.test",
        "!help",
        None,
        MatrixBotCommandContext::default(),
        &acl,
    ));
    assert_eq!(public_help.tier, MatrixBotCommandTier::Public);
    assert!(public_help.authorization.allowed);
    assert_eq!(
        public_help.authorization.reason,
        MatrixBotCommandAuthReason::Public
    );

    let denied_status = command(plan_matrix_bot_command(
        "@guest:matrix.test",
        "!status",
        None,
        MatrixBotCommandContext::default(),
        &acl,
    ));
    assert_eq!(denied_status.tier, MatrixBotCommandTier::OperatorRead);
    assert!(!denied_status.authorization.allowed);
    assert_eq!(
        denied_status.authorization.reason,
        MatrixBotCommandAuthReason::OperatorRequired
    );

    let operator_status = command(plan_matrix_bot_command(
        "@operator:matrix.test",
        "!status",
        None,
        MatrixBotCommandContext::default(),
        &acl,
    ));
    assert!(operator_status.authorization.allowed);
    assert_eq!(
        operator_status.authorization.reason,
        MatrixBotCommandAuthReason::Operator
    );

    let operator_dm = command(plan_matrix_bot_command(
        "@operator:matrix.test",
        "!dm codex-worker",
        None,
        MatrixBotCommandContext::default(),
        &acl,
    ));
    assert_eq!(operator_dm.tier, MatrixBotCommandTier::OperatorManagement);
    assert!(operator_dm.authorization.allowed);

    let operator_spy = command(plan_matrix_bot_command(
        "@operator:matrix.test",
        "!spy codex-worker codex-reviewer",
        None,
        MatrixBotCommandContext::default(),
        &acl,
    ));
    assert_eq!(operator_spy.tier, MatrixBotCommandTier::Admin);
    assert!(!operator_spy.authorization.allowed);
    assert_eq!(
        operator_spy.authorization.reason,
        MatrixBotCommandAuthReason::AdminRequired
    );

    let admin_spy = command(plan_matrix_bot_command(
        "@admin:matrix.test",
        "!spy codex-worker codex-reviewer",
        None,
        MatrixBotCommandContext::default(),
        &acl,
    ));
    assert!(admin_spy.authorization.allowed);
    assert_eq!(
        admin_spy.authorization.reason,
        MatrixBotCommandAuthReason::Admin
    );

    let unknown = command(plan_matrix_bot_command(
        "@operator:matrix.test",
        "!unknown",
        None,
        MatrixBotCommandContext::default(),
        &acl,
    ));
    assert_eq!(unknown.tier, MatrixBotCommandTier::OperatorRead);

    let no_acl_unknown = command(plan_matrix_bot_command(
        "@guest:matrix.test",
        "!unknown",
        None,
        MatrixBotCommandContext::default(),
        &MatrixBotCommandAcl::default(),
    ));
    assert_eq!(no_acl_unknown.tier, MatrixBotCommandTier::OperatorRead);
    assert!(no_acl_unknown.authorization.allowed);
    assert_eq!(
        no_acl_unknown.authorization.reason,
        MatrixBotCommandAuthReason::NoAcl
    );
}

#[test]
fn matrix_bot_command_planner_returns_fallback_for_non_commands() {
    let fallback = fallback(plan_matrix_bot_command(
        "@alex:matrix.test",
        "show me what you can do",
        None,
        MatrixBotCommandContext::default(),
        &MatrixBotCommandAcl::default(),
    ));

    assert_eq!(fallback.reply_hint, "Send !help for available commands.");
    assert!(fallback.command.is_none());
    assert!(fallback.args.is_empty());
}

#[test]
fn matrix_bot_command_executor_replies_to_help_status_agents_and_groups() {
    let acl = MatrixBotCommandAcl::default();
    let snapshot = snapshot();

    let help = execute_matrix_bot_command(
        &command_in_room("@alex:matrix.test", "!help", &acl),
        &snapshot,
    )
    .expect("help execution");
    let status = execute_matrix_bot_command(
        &command_in_room("@alex:matrix.test", "!status", &acl),
        &snapshot,
    )
    .expect("status execution");
    let online_agents = execute_matrix_bot_command(
        &command_in_room("@alex:matrix.test", "!agents", &acl),
        &snapshot,
    )
    .expect("agents execution");
    let all_agents = execute_matrix_bot_command(
        &command_in_room("@alex:matrix.test", "!agents all", &acl),
        &snapshot,
    )
    .expect("agents all execution");
    let groups = execute_matrix_bot_command(
        &command_in_room("@alex:matrix.test", "!groups", &acl),
        &snapshot,
    )
    .expect("groups execution");

    assert_eq!(help.reply.room_id, "!bot:matrix.test");
    assert!(
        help.reply
            .body
            .contains("=== Agent Bridge Bot Commands ===")
    );
    assert!(help.reply.body.contains("!status"));
    assert!(status.reply.body.contains("=== System Status ==="));
    assert!(status.reply.body.contains("Agents: 2"));
    assert!(status.reply.body.contains("Groups: 1"));
    assert!(status.reply.body.contains("Tmux sessions: unavailable"));
    assert!(status.reply.body.contains("Bridge: running"));
    assert!(online_agents.reply.body.contains("=== Online Agents ==="));
    assert!(online_agents.reply.body.contains("codex-worker"));
    assert!(!online_agents.reply.body.contains("codex-reviewer"));
    assert!(
        online_agents
            .reply
            .body
            .contains("Use !agents all to see all agents including offline.")
    );
    assert!(all_agents.reply.body.contains("=== All Agents ==="));
    assert!(all_agents.reply.body.contains("codex-reviewer"));
    assert!(groups.reply.body.contains("=== Groups ==="));
    assert!(
        groups
            .reply
            .body
            .contains("ops (2 members): codex-worker, codex-reviewer")
    );
}

#[test]
fn matrix_bot_command_executor_rejects_unauthorized_commands_without_side_effects() {
    let acl = MatrixBotCommandAcl {
        operator_mxids: vec!["@operator:matrix.test".to_owned()],
        admin_mxids: Vec::new(),
    };
    let command = command_in_room("@guest:matrix.test", "!status", &acl);

    let execution = execute_matrix_bot_command(&command, &snapshot()).expect("execution");

    assert_eq!(execution.reply.room_id, "!bot:matrix.test");
    assert_eq!(
        execution.reply.body,
        "Access denied: !status requires operator privileges."
    );
    assert!(
        execution.side_effects.is_empty(),
        "ACL denial must not declare side effects"
    );
}

#[test]
fn matrix_bot_command_executor_answers_unknown_and_unsupported_commands_without_mutation() {
    let acl = MatrixBotCommandAcl::default();
    let unknown = execute_matrix_bot_command(
        &command_in_room("@alex:matrix.test", "!doesnotexist", &acl),
        &snapshot(),
    )
    .expect("unknown command execution");
    let unsupported = execute_matrix_bot_command(
        &command_in_room("@alex:matrix.test", "!dm codex-worker", &acl),
        &snapshot(),
    )
    .expect("unsupported command execution");

    assert!(
        unknown
            .reply
            .body
            .contains("Unknown command: !doesnotexist")
    );
    assert!(
        unknown
            .reply
            .body
            .contains("Send !help for available commands.")
    );
    assert!(
        unsupported
            .reply
            .body
            .contains("Command not implemented in agentd Matrix bridge yet: !dm")
    );
    for execution in [unknown, unsupported] {
        assert!(
            execution.side_effects.is_empty(),
            "unknown or unsupported command must not declare side effects"
        );
    }
}

#[test]
fn matrix_bot_command_executor_with_effects_executes_dm_invite_request() {
    let acl = MatrixBotCommandAcl::default();
    let mut backend = FakeManagementBackend {
        agents: vec![snapshot().agents[0].clone()],
        ..FakeManagementBackend::default()
    };
    let mut rooms = FakeRoomEffects {
        dm_result: MatrixBotDmRoomResult {
            room_id: Some("!dm-codex-worker:matrix.test".to_owned()),
            human_status: MatrixBotDmRoomStatus::Invited,
            invite_error: None,
        },
        ..FakeRoomEffects::default()
    };
    let command = command_in_room("@alex:matrix.test", "!dm codex-worker", &acl);

    let execution =
        execute_matrix_bot_command_with_effects(&command, &snapshot(), &mut backend, &mut rooms)
            .expect("dm execution");

    assert_eq!(backend.lookups, ["codex-worker"]);
    assert_eq!(
        rooms.dm_requests,
        [("codex-worker".to_owned(), "@alex:matrix.test".to_owned())]
    );
    assert!(
        execution
            .reply
            .body
            .contains("DM room ready for codex-worker")
    );
    assert!(execution.reply.body.contains("Invite sent"));
    assert!(
        execution
            .reply
            .body
            .contains("https://matrix.to/#/!dm-codex-worker:matrix.test")
    );
    assert_eq!(
        execution.side_effects,
        [MatrixBotCommandSideEffect::ChangesMatrixRooms]
    );
}

#[test]
fn matrix_bot_command_executor_with_effects_updates_identity_from_context_or_args() {
    let acl = MatrixBotCommandAcl::default();
    let mut backend = FakeManagementBackend {
        identity_result: MatrixBotCommandMutationResult::ok(),
        ..FakeManagementBackend::default()
    };
    let mut rooms = FakeRoomEffects::default();
    let direct_context = MatrixBotCommandContext {
        group_name: None,
        target_agent: Some("codex-worker".to_owned()),
    };
    let direct = command_in_room_with_context(
        "@alex:matrix.test",
        "!identity Be concise",
        direct_context,
        &acl,
    );
    let explicit = command_in_room(
        "@alex:matrix.test",
        "!identity codex-reviewer Review carefully",
        &acl,
    );

    let direct_execution =
        execute_matrix_bot_command_with_effects(&direct, &snapshot(), &mut backend, &mut rooms)
            .expect("direct identity execution");
    let explicit_execution =
        execute_matrix_bot_command_with_effects(&explicit, &snapshot(), &mut backend, &mut rooms)
            .expect("explicit identity execution");

    assert_eq!(
        backend.identity_updates,
        [
            ("codex-worker".to_owned(), "Be concise".to_owned()),
            ("codex-reviewer".to_owned(), "Review carefully".to_owned())
        ]
    );
    assert!(
        direct_execution
            .reply
            .body
            .contains("Identity set for codex-worker: Be concise")
    );
    assert!(
        explicit_execution
            .reply
            .body
            .contains("Identity set for codex-reviewer: Review carefully")
    );
    assert_eq!(
        direct_execution.side_effects,
        [MatrixBotCommandSideEffect::MutatesBackend]
    );
    assert_eq!(
        explicit_execution.side_effects,
        [MatrixBotCommandSideEffect::MutatesBackend]
    );
}

#[test]
fn matrix_bot_command_executor_with_effects_creates_group() {
    let acl = MatrixBotCommandAcl::default();
    let mut backend = FakeManagementBackend {
        group_result: MatrixBotCommandMutationResult::ok(),
        ..FakeManagementBackend::default()
    };
    let mut rooms = FakeRoomEffects::default();
    let command = command_in_room(
        "@alex:matrix.test",
        "!mkgroup ops codex-worker codex-reviewer",
        &acl,
    );

    let execution =
        execute_matrix_bot_command_with_effects(&command, &snapshot(), &mut backend, &mut rooms)
            .expect("mkgroup execution");

    assert_eq!(
        backend.group_creates,
        [(
            "ops".to_owned(),
            vec!["codex-worker".to_owned(), "codex-reviewer".to_owned()]
        )]
    );
    assert_eq!(
        execution.reply.body,
        "Group \"ops\" created with members: codex-worker, codex-reviewer"
    );
    assert_eq!(
        execution.side_effects,
        [MatrixBotCommandSideEffect::MutatesBackend]
    );
}

#[test]
fn matrix_bot_command_executor_with_effects_updates_group_members() {
    let acl = MatrixBotCommandAcl::default();
    let mut backend = FakeManagementBackend {
        group_result: MatrixBotCommandMutationResult::ok(),
        ..FakeManagementBackend::default()
    };
    let mut rooms = FakeRoomEffects::default();
    let add = command_in_room("@alex:matrix.test", "!addmember ops codex-reviewer", &acl);
    let remove = command_in_room_with_context(
        "@alex:matrix.test",
        "!rmember codex-worker",
        MatrixBotCommandContext {
            group_name: Some("ops".to_owned()),
            target_agent: None,
        },
        &acl,
    );

    let add_execution =
        execute_matrix_bot_command_with_effects(&add, &snapshot(), &mut backend, &mut rooms)
            .expect("addmember execution");
    let remove_execution =
        execute_matrix_bot_command_with_effects(&remove, &snapshot(), &mut backend, &mut rooms)
            .expect("rmember execution");

    assert_eq!(
        backend.group_member_updates,
        [
            (
                "ops".to_owned(),
                vec!["codex-reviewer".to_owned()],
                Vec::<String>::new()
            ),
            (
                "ops".to_owned(),
                Vec::<String>::new(),
                vec!["codex-worker".to_owned()]
            )
        ]
    );
    assert_eq!(add_execution.reply.body, "Added codex-reviewer to ops");
    assert_eq!(remove_execution.reply.body, "Removed codex-worker from ops");
    assert_eq!(
        add_execution.side_effects,
        [MatrixBotCommandSideEffect::MutatesBackend]
    );
    assert_eq!(
        remove_execution.side_effects,
        [MatrixBotCommandSideEffect::MutatesBackend]
    );
}

#[test]
fn matrix_bot_command_executor_with_effects_removes_group() {
    let acl = MatrixBotCommandAcl::default();
    let mut backend = FakeManagementBackend {
        groups: vec![MatrixBotGroupSummary {
            name: "ops".to_owned(),
            members: vec!["codex-worker".to_owned()],
        }],
        group_result: MatrixBotCommandMutationResult::ok(),
        ..FakeManagementBackend::default()
    };
    let mut rooms = FakeRoomEffects::default();
    let command = command_in_room("@alex:matrix.test", "!rmgroup ops", &acl);

    let execution =
        execute_matrix_bot_command_with_effects(&command, &snapshot(), &mut backend, &mut rooms)
            .expect("rmgroup execution");

    assert_eq!(backend.group_lookups, ["ops"]);
    assert_eq!(backend.group_deletes, ["ops"]);
    assert!(execution.reply.body.contains("Group \"ops\" removed"));
    assert!(
        execution
            .reply
            .body
            .contains("Matrix room cleanup is not included in p261")
    );
    assert_eq!(
        execution.side_effects,
        [MatrixBotCommandSideEffect::MutatesBackend]
    );
}

#[test]
fn matrix_bot_command_executor_with_effects_joins_group() {
    let acl = MatrixBotCommandAcl::default();
    let mut backend = FakeManagementBackend {
        group_result: MatrixBotCommandMutationResult::ok(),
        ..FakeManagementBackend::default()
    };
    let mut rooms = FakeRoomEffects {
        group_room_result: MatrixBotGroupRoomResult {
            room_id: Some("!ops:matrix.test".to_owned()),
            human_status: MatrixBotDmRoomStatus::Invited,
            invite_error: None,
        },
        ..FakeRoomEffects::default()
    };
    let command = command_in_room("@alex:matrix.test", "!joingroup ops", &acl);

    let execution =
        execute_matrix_bot_command_with_effects(&command, &snapshot(), &mut backend, &mut rooms)
            .expect("joingroup execution");

    assert_eq!(
        backend.group_member_updates,
        [(
            "ops".to_owned(),
            vec!["alex".to_owned()],
            Vec::<String>::new()
        )]
    );
    assert_eq!(
        rooms.group_room_requests,
        [("ops".to_owned(), "@alex:matrix.test".to_owned())]
    );
    assert!(
        execution
            .reply
            .body
            .contains("Added you (alex) to group \"ops\"")
    );
    assert!(execution.reply.body.contains("Invite sent"));
    assert_eq!(
        execution.side_effects,
        [
            MatrixBotCommandSideEffect::MutatesBackend,
            MatrixBotCommandSideEffect::ChangesMatrixRooms
        ]
    );
}

#[test]
fn matrix_bot_command_executor_with_effects_joins_contextual_group() {
    let acl = MatrixBotCommandAcl::default();
    let mut backend = FakeManagementBackend {
        group_result: MatrixBotCommandMutationResult::ok(),
        ..FakeManagementBackend::default()
    };
    let mut rooms = FakeRoomEffects {
        group_room_result: MatrixBotGroupRoomResult {
            room_id: Some("!ops:matrix.test".to_owned()),
            human_status: MatrixBotDmRoomStatus::Joined,
            invite_error: None,
        },
        ..FakeRoomEffects::default()
    };
    let command = command_in_room_with_context(
        "@alex:matrix.test",
        "!joingroup",
        MatrixBotCommandContext {
            group_name: Some("ops".to_owned()),
            target_agent: None,
        },
        &acl,
    );

    let execution =
        execute_matrix_bot_command_with_effects(&command, &snapshot(), &mut backend, &mut rooms)
            .expect("contextual joingroup execution");

    assert_eq!(
        backend.group_member_updates,
        [(
            "ops".to_owned(),
            vec!["alex".to_owned()],
            Vec::<String>::new()
        )]
    );
    assert_eq!(
        rooms.group_room_requests,
        [("ops".to_owned(), "@alex:matrix.test".to_owned())]
    );
    assert!(
        execution
            .reply
            .body
            .contains("Added you (alex) to group \"ops\"")
    );
}

#[test]
fn matrix_bot_command_executor_with_effects_rejects_invalid_joingroup() {
    let acl = MatrixBotCommandAcl::default();
    let mut backend = FakeManagementBackend::default();
    let mut rooms = FakeRoomEffects::default();

    let missing_group = execute_matrix_bot_command_with_effects(
        &command_in_room("@alex:matrix.test", "!joingroup", &acl),
        &snapshot(),
        &mut backend,
        &mut rooms,
    )
    .expect("missing joingroup execution");
    assert_eq!(
        missing_group.reply.body,
        "Usage: !joingroup <group> (or use inside a group room)"
    );
    assert!(missing_group.side_effects.is_empty());

    backend.group_result = MatrixBotCommandMutationResult::failed("group_not_found");
    let failed_backend = execute_matrix_bot_command_with_effects(
        &command_in_room("@alex:matrix.test", "!joingroup ops", &acl),
        &snapshot(),
        &mut backend,
        &mut rooms,
    )
    .expect("failed joingroup execution");
    assert_eq!(failed_backend.reply.body, "Failed: group_not_found");
    assert_eq!(
        backend.group_member_updates,
        [(
            "ops".to_owned(),
            vec!["alex".to_owned()],
            Vec::<String>::new()
        )]
    );
    assert!(rooms.group_room_requests.is_empty());
}

#[test]
fn matrix_bot_command_executor_with_effects_rejects_invalid_group_management() {
    let acl = MatrixBotCommandAcl::default();
    let mut backend = FakeManagementBackend {
        group_result: MatrixBotCommandMutationResult::failed("group_not_found"),
        ..FakeManagementBackend::default()
    };
    let mut rooms = FakeRoomEffects::default();

    let missing_name = execute_matrix_bot_command_with_effects(
        &command_in_room("@alex:matrix.test", "!mkgroup", &acl),
        &snapshot(),
        &mut backend,
        &mut rooms,
    )
    .expect("missing name execution");
    let bad_add = execute_matrix_bot_command_with_effects(
        &command_in_room("@alex:matrix.test", "!addmember ops", &acl),
        &snapshot(),
        &mut backend,
        &mut rooms,
    )
    .expect("bad add execution");
    let failed_add = execute_matrix_bot_command_with_effects(
        &command_in_room("@alex:matrix.test", "!addmember ops codex-reviewer", &acl),
        &snapshot(),
        &mut backend,
        &mut rooms,
    )
    .expect("failed add execution");
    let unknown_remove = execute_matrix_bot_command_with_effects(
        &command_in_room("@alex:matrix.test", "!rmgroup ghost", &acl),
        &snapshot(),
        &mut backend,
        &mut rooms,
    )
    .expect("unknown group execution");

    assert_eq!(
        missing_name.reply.body,
        "Usage: !mkgroup <name> [member1] [member2] ..."
    );
    assert_eq!(
        bad_add.reply.body,
        "Usage: !addmember <group> <name> (or !addmember <name> inside a group room)"
    );
    assert_eq!(failed_add.reply.body, "Failed: group_not_found");
    assert_eq!(unknown_remove.reply.body, "Group not found: ghost");
    assert!(missing_name.side_effects.is_empty());
    assert!(bad_add.side_effects.is_empty());
    assert!(unknown_remove.side_effects.is_empty());
    assert_eq!(
        failed_add.side_effects,
        [MatrixBotCommandSideEffect::MutatesBackend]
    );
    assert_eq!(backend.group_deletes, Vec::<String>::new());
}

#[test]
fn matrix_bot_command_executor_with_effects_handles_management_errors() {
    let acl = MatrixBotCommandAcl::default();

    let mut backend = FakeManagementBackend::default();
    let mut rooms = FakeRoomEffects::default();
    let missing_agent = execute_matrix_bot_command_with_effects(
        &command_in_room("@alex:matrix.test", "!dm", &acl),
        &snapshot(),
        &mut backend,
        &mut rooms,
    )
    .expect("missing agent usage");
    assert_eq!(missing_agent.reply.body, "Usage: !dm <agent>");
    assert!(backend.lookups.is_empty());
    assert!(rooms.dm_requests.is_empty());
    assert!(missing_agent.side_effects.is_empty());

    let ghost = execute_matrix_bot_command_with_effects(
        &command_in_room("@alex:matrix.test", "!dm ghost", &acl),
        &snapshot(),
        &mut backend,
        &mut rooms,
    )
    .expect("ghost agent");
    assert_eq!(backend.lookups, ["ghost"]);
    assert!(rooms.dm_requests.is_empty());
    assert_eq!(ghost.reply.body, "Agent not found: ghost");
    assert!(ghost.side_effects.is_empty());

    let bad_identity = execute_matrix_bot_command_with_effects(
        &command_in_room("@alex:matrix.test", "!identity terse", &acl),
        &snapshot(),
        &mut backend,
        &mut rooms,
    )
    .expect("identity usage");
    assert_eq!(
        bad_identity.reply.body,
        "Usage: !identity <text> (in agent DM) or !identity <agent> <text>"
    );
    assert!(bad_identity.side_effects.is_empty());

    let mut failing_backend = FakeManagementBackend {
        identity_result: MatrixBotCommandMutationResult::failed("backend denied identity update"),
        ..FakeManagementBackend::default()
    };
    let failure = execute_matrix_bot_command_with_effects(
        &command_in_room(
            "@alex:matrix.test",
            "!identity codex-worker Speak clearly",
            &acl,
        ),
        &snapshot(),
        &mut failing_backend,
        &mut rooms,
    )
    .expect("identity failure");
    assert!(
        failure
            .reply
            .body
            .contains("Failed: backend denied identity update")
    );
    assert_eq!(
        failure.side_effects,
        [MatrixBotCommandSideEffect::MutatesBackend]
    );
}
