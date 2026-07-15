spec: task
name: "agent-chat Matrix bot command planner"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, bot-commands, phase-g, p255]
---

## Intent

Move the Matrix bridge replacement path past plain text relay by adding a
tested agent-chat-compatible bot command parsing and authorization planning
surface. This slice makes agentd recognize the command grammar that
agent-chat's Matrix bot exposes, but it does not execute the commands yet.

## Decisions

- Add a pure `agentd-matrix` bot command planner that can be tested without a
  Matrix homeserver, backend, daemon, tmux, or real agent runtime.
- Parse bang-prefixed commands with agent-chat-compatible whitespace handling:
  trim the message, require `!`, lowercase only the command token, and preserve
  argument tokens in order.
- Strip Matrix mention-pill command prefixes when the formatted body starts
  with a Matrix user mention and the plain body contains a later `!` command,
  matching the bridge-side behavior in agent-chat.
- Classify the agent-chat command set with the same tiers:
  public `!help`; operator read commands `!status`, `!agents`, `!groups`,
  `!group`, `!agent`, `!sessions`, `!mcp`, and `!bridge`; operator management
  commands `!mkgroup`, `!addmember`, `!rmember`, `!joingroup`, `!dm`,
  `!identity`, and `!rmgroup`; admin commands `!spy`, `!agentctl`, and `!ctl`;
  and unknown bang commands defaulting to operator read tier.
- Return a command plan that includes command, args, tier, authorization
  decision, sender human localpart, and room context (`group_name` and
  `target_agent`).
- Preserve agent-chat ACL compatibility: `!help` is public, configured admins
  may run all tiers, configured operators may run operator tiers, admin-only
  commands require an admin, and an empty ACL allows all commands for backward
  compatibility.
- Return a non-command fallback plan with the agent-chat-compatible reply hint
  `Send !help for available commands.` so bot-DM fallback behavior is explicit.
- Keep command execution, backend mutation, Matrix sends, room lifecycle
  changes, tmux pane control, identity updates, media transfer, service
  packaging, cutover, rollback, and dashboard rendering out of this slice.

## Boundaries

### Allowed Changes

- specs/e2e/p255-agent-chat-matrix-bot-command-planner.spec.md
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/bot_commands.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not connect to a real Matrix homeserver in tests.
- Do not start a real agentd daemon in tests.
- Do not execute bot commands against the backend, tmux, Matrix rooms, or
  agent runtimes.
- Do not add command-response rendering beyond the non-command fallback hint.
- Do not add new Rust dependencies.
- Do not change the p236 agentd HTTP Matrix contract.
- Do not enable `matrix-sdk-adapter` by default.
- Do not add Matrix media transfer, room lifecycle execution, service
  packaging, cutover, rollback, dashboard rendering, token rotation, or Matrix
  profile/avatar sync.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

Bot command execution, Matrix message sends, room creation/deletion, group
membership mutation, agent DM creation, identity writes, spy rooms, agentctl/tmux
control, backend API mutation, real Matrix homeserver validation, media
transfer, service packaging, operator cutover, rollback automation, dashboard
rendering, Matrix profile/avatar updates, token rotation, and remote relay
service packaging.

## Completion Criteria

<!-- lint-ack: decision-coverage - p255 binds command grammar, mention-prefix stripping, command tier classification, ACL behavior, context preservation, fallback handling, docs, and no execution through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify pure Rust parser/planner return values and repository Markdown/source state without a Matrix homeserver or daemon. -->
<!-- lint-ack: error-path - p255 covers non-command fallback, operator denial, and admin-only denial paths. -->
<!-- lint-ack: boundary-entry-point - p255 touches one library entry point and one test entry point; scenarios reference both. -->

Scenario: parser recognizes agent-chat bang commands and preserves arguments
  Test:
    Package: agentd-matrix
    Filter: matrix_bot_command_parser_recognizes_bang_commands_and_preserves_args
  Level: unit
  Test Double: pure Rust values
  Given a Matrix message body with surrounding whitespace and `!IDENTITY codex-worker Be concise`
  When the bot command planner parses the body
  Then the command is `!identity`
  And the args are `codex-worker`, `Be`, and `concise`
  And the tier is operator management
  And the sender human localpart is derived from the sender MXID
  And the room context preserves `group_name` and `target_agent`

Scenario: parser strips Matrix mention prefix from formatted command bodies
  Test:
    Package: agentd-matrix
    Filter: matrix_bot_command_parser_strips_matrix_mention_prefix
  Level: unit
  Test Double: pure Rust values
  Given a plain body `Agent Bridge: !status`
  And a formatted body that starts with a Matrix mention pill followed by `: !status`
  When the bot command planner parses the message
  Then the command is `!status`
  And the args are empty
  And the tier is operator read

Scenario: ACL mirrors agent-chat public operator admin and empty-acl behavior
  Test:
    Package: agentd-matrix
    Filter: matrix_bot_command_acl_matches_agent_chat_tiers
  Level: unit
  Test Double: pure Rust values
  Given operator and admin MXID lists are configured
  When `!help`, `!status`, `!dm`, `!spy`, and `!unknown` are planned for public, operator, and admin senders
  Then `!help` is public
  And operator senders can run operator read and management commands
  And operator senders cannot run admin commands
  And admin senders can run admin commands
  And unknown bang commands default to operator read tier
  And an empty ACL allows unknown bang commands for backward compatibility

Scenario: non-command input returns the agent-chat fallback hint without execution
  Test:
    Package: agentd-matrix
    Filter: matrix_bot_command_planner_returns_fallback_for_non_commands
  Level: unit
  Test Double: pure Rust values
  Given a non-command bot-DM message body
  When the bot command planner handles the message
  Then it returns a non-command plan
  And the fallback reply is `Send !help for available commands.`
  And no command name, args, or execution target is produced

Scenario: parity docs record p255 bot command planner without declaring replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p255_matrix_bot_command_planner_progress
  Level: artifact inspection
  Test Double: repository Markdown files and Rust source text
  Given `crates/agentctl/tests/parity_cli.rs` inspects the agent-chat replacement parity map, roadmap, and `agentd-matrix` source
  When p255 progress is inspected
  Then the Matrix bridge row mentions p255 and bot command planner progress
  And the Matrix bridge row remains partial
  And the row still names Matrix media, service packaging, cutover, rollback, token rotation, bridge operations, and dashboard/operator visibility gaps
  And the roadmap mentions p255 and bot command planner progress
  And `crates/agentd-matrix/src/lib.rs` mentions `MatrixBotCommandPlan`, `MatrixBotCommandAcl`, and `Send !help for available commands.`
