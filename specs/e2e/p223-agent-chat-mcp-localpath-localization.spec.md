spec: task
name: "agent-chat MCP LocalPath localization baseline"
tags: [agent-chat-replacement, messaging, attachments, media, mcp, proxy, phase-d, p223]
---

## Intent

Advance `attachments_media` after p222 by adding the first agent-chat-style MCP
media localization path for remote/isolated agents. When an agentd stdio MCP
session proxies `check_inbox` or `check_group` through a daemon, returned staged
media paths are fetched through `/api/media/fetch`, cached on the agent side,
and returned as local paths the agent can read. Matrix transfer, remote relay
sync, image sanitization, dashboard previews, and import/cutover remain outside
this slice.

## Decisions

- Localization belongs in the stdio MCP proxy path, not in the durable message
  store or surface tools, because only the proxy knows which filesystem is local
  to the receiving agent.
- Only proxied `check_inbox` and `check_group` responses are localized in p223.
  Direct in-process stdio sessions keep existing local path behavior.
- Attachment objects with a non-empty `path` are localized by first reusing a
  readable local file if present; otherwise the proxy fetches
  `/api/media/fetch?path=<source>` from the daemon, writes the bytes into a local
  media cache, and rewrites the attachment `path`.
- Message `summary` and `full` text lines matching `LocalPath: <source>` are
  localized the same way, rewritten to `LocalPath: <local-cache-path>`, and
  merged into the message attachments.
- Cache paths are deterministic by source path, preserve a sanitized display
  name and extension when available, and reuse warm cached files without a
  second daemon media fetch.
- The testable cache root may be injected by tests; production proxy sessions
  use an environment/default cache root without requiring new CLI flags.
- Fetch failures must not fail `check_inbox` or `check_group`; they leave the
  original message/attachment in place and add a warning field for visibility.
- The parity map keeps `attachments_media` partial until Matrix media transfer,
  remote relay media handling, image sanitization, dashboard previews, and
  import/cutover are done.

## Boundaries

### Allowed Changes

- specs/e2e/p223-agent-chat-mcp-localpath-localization.spec.md
- crates/agentd-bin/src/stdio_mcp.rs
- crates/agentd-bin/tests/mcp_stdio.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not modify the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout or any source JSON file.
- Do not start Matrix, tmux, Claude, systemd, launchd, remote relay, or real
  media bridge processes in automated tests.
- Do not implement Matrix media upload/download, remote relay media sync, image
  sanitization, dashboard previews, or import/cutover in this slice.
- Do not add new CLI flags or change the public MCP tool schemas.
- Do not add a database table for media blobs.
- Do not change durable message truth when localizing proxy responses.
- Do not claim full agent-chat replacement after this slice.

## Out of Scope

- Matrix bridge media upload/download and trust policy.
- Remote relay media sync and cross-host delivery guarantees.
- Image validation, resizing, conversion, thumbnailing, or MIME sniffing from
  file bytes.
- Dashboard message/attachment pages.
- Importing existing agent-chat media files or attachment cursors.

## Completion Criteria

<!-- lint-ack: decision-coverage - p223 binds proxy-only localization, attachment fetch/cache, LocalPath text rewrite, warm cache reuse, fetch-failure fallback, unchanged schemas, docs, and parity through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios cover MCP JSON-RPC structuredContent, daemon HTTP requests, cache file side effects, warm cache behavior, error fallback, schema output, and docs. -->

Scenario: proxy check_inbox localizes staged attachment paths
  Test:
    Package: agentd-bin
    Filter: mcp_stdio_proxy_check_inbox_localizes_staged_attachments
  Level: stdio MCP proxy integration
  Test Double: in-process test TCP daemon and temp media cache
  Given a proxied `check_inbox` response with one staged attachment path
  When the proxy handles `tools/call` for `check_inbox`
  Then it first posts `/tools/call` to the daemon
  And it fetches the attachment bytes through `/api/media/fetch`
  And the returned `structuredContent.dm[0].attachments[0].path` points inside
  the local cache
  And that local cache file contains the fetched bytes
  And `source_path`, `name`, `mime`, `kind`, `size`, and `staged=true` remain
  visible to the agent

Scenario: proxy check_group localizes LocalPath text and merges attachments
  Test:
    Package: agentd-bin
    Filter: mcp_stdio_proxy_check_group_localizes_localpath_lines
  Level: stdio MCP proxy integration
  Test Double: in-process test TCP daemon and temp media cache
  Given a proxied `check_group` response whose unread message `full` contains
  `LocalPath: <daemon media path>`
  When the proxy handles `tools/call` for `check_group`
  Then the `LocalPath:` line is rewritten to a local cache path
  And the message attachments include the localized file metadata
  And the cache file contains the fetched bytes

Scenario: proxy media localization reuses warm cache
  Test:
    Package: agentd-bin
    Filter: mcp_stdio_proxy_media_localization_reuses_warm_cache
  Level: stdio MCP proxy integration
  Test Double: in-process test TCP daemon and temp media cache
  Given the cache already contains the deterministic local file for a daemon
  media path
  When the proxy handles `check_inbox` with the same staged attachment path
  Then it does not request `/api/media/fetch`
  And the returned attachment path is the existing cached file

Scenario: proxy media localization keeps message on fetch failure
  Test:
    Package: agentd-bin
    Filter: mcp_stdio_proxy_media_localization_warns_without_failing_on_fetch_error
  Level: stdio MCP proxy integration
  Test Double: in-process test TCP daemon and temp media cache
  Given `/api/media/fetch` returns an error for one staged attachment
  When the proxy handles `check_inbox`
  Then JSON-RPC still returns a successful `check_inbox` result
  And the attachment path remains the original daemon path
  And the message includes a media warning mentioning the failed path

Scenario: proxy media localization does not change public tool schemas
  Test:
    Package: agentd-bin
    Filter: mcp_stdio_proxy_media_localization_keeps_tool_schemas_unchanged
  Level: stdio JSON-RPC unit
  Test Double: production host with temp SQLite and fake backend
  Given a stdio MCP handler
  When `tools/list` is requested
  Then `check_inbox.inputSchema.properties.attachments` is still absent
  And `check_group.inputSchema.properties.attachments` is still absent
  And no new MCP media upload tool is listed

Scenario: parity map records p223 LocalPath localization progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p223_localpath_localization_progress
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the agent-chat replacement parity map
  When the `attachments_media` row is inspected
  Then it mentions p223 LocalPath localization and stdio MCP proxy media cache
  And the row remains partial because Matrix media transfer, remote relay media,
  image sanitization, dashboard previews, and import/cutover are not complete
