spec: task
name: "agent-chat media stage fetch baseline"
tags: [agent-chat-replacement, messaging, attachments, media, http, phase-d, p222]
---

## Intent

Advance `attachments_media` beyond p221 metadata by adding the first local media
byte path compatible with agent-chat's `/api/media/stage` and
`/api/media/fetch` behavior. Agents can stage bytes into an agentd-controlled
media directory, persist the returned staged attachment metadata on messages,
and fetch those bytes later by the staged path. Matrix media transfer, remote
relay media handling, image sanitization, dashboard previews, and import/cutover
remain outside this slice.

## Decisions

- Add an explicit HTTP media configuration to the surface state with one
  `media_dir` root.
- Daemon production routers use a media directory next to the configured
  SQLite database: `<db parent>/media`.
- `POST /api/media/stage` accepts `from`, `content_base64`, optional
  `source_path`, `name`, `mime`, and `kind`.
- Staging requires an existing registered agent and enforces the existing
  agent-token policy for that `from` agent.
- Staging decodes base64, rejects empty payloads, rejects payloads larger than
  `20 MiB`, writes bytes under `media_dir`, and returns
  `{ok:true, attachment}`.
- Staged attachment metadata keeps agent-chat fields: `path`, `name`, `mime`,
  `kind`, `size`, `staged=true`, and `source_path`.
- `GET /api/media/fetch?path=...` only serves regular, non-empty files whose
  resolved path is inside `media_dir`; path escape returns `403`, missing files
  return `404`, and oversized files return `413`.
- Fetch responses return the original bytes with `Content-Type`,
  `Content-Length`, and inline `Content-Disposition` headers.
- `POST /api/messages` preserves `staged=true` attachment metadata when the
  staged path is inside `media_dir`; unstaged local-file attachments still use
  p221 validation and persist `staged=false`.
- MCP `send_message` and `post` remain local-file metadata tools in this slice;
  a dedicated MCP media staging tool is out of scope.
- The parity map keeps `attachments_media` partial until Matrix transfer,
  remote relay media handling, LocalPath localization, sanitization, dashboard
  previews, and import/cutover are done.

## Boundaries

### Allowed Changes

- **/Cargo.toml
- crates/agentd-surface/Cargo.toml
- specs/e2e/p222-agent-chat-media-stage-fetch.spec.md
- specs/e2e/p221-agent-chat-attachment-metadata-baseline.spec.md
- crates/agentd-surface/src/http.rs
- crates/agentd-surface/src/tools/attachments.rs
- crates/agentd-surface/src/tools/post.rs
- crates/agentd-surface/tests/http.rs
- crates/agentd-surface/tests/runs_overview.rs
- crates/agentd-bin/src/daemon.rs
- crates/agentd-bin/tests/daemon_http.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not modify the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout or any source JSON file.
- Do not start Matrix, tmux, Claude, systemd, launchd, remote relay, or real
  media bridge processes in automated tests.
- Do not implement Matrix media upload/download, remote relay media handling,
  LocalPath localization, image sanitization, dashboard previews, or
  import/cutover in this slice.
- Do not add a database table for media blobs in this slice.
- Do not let `/api/media/fetch` serve arbitrary filesystem paths.
- Do not change MCP `send_message` or `post` into byte-upload tools.
- Do not claim full agent-chat replacement after this slice.

## Out of Scope

- Matrix bridge media upload/download and trust policy.
- Remote relay media sync and cross-host fetch.
- Image sanitization and thumbnail/preview generation.
- Dashboard message/attachment pages.
- Importing existing agent-chat media files or attachment cursors.

## Completion Criteria

<!-- lint-ack: decision-coverage - p222 binds media config, stage/fetch success, agent-token/unknown-agent failures, path escape, staged metadata preservation, production rebuild persistence, docs, and parity through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios cover HTTP status, headers, bytes, persisted message JSON, production directory reuse, and docs. -->

Scenario: HTTP media stage writes bytes and fetch returns them
  Test:
    Package: agentd-surface
    Filter: http_media_stage_writes_bytes_and_fetch_returns_same_content
  Level: HTTP integration
  Test Double: FakeRunHost, temp media directory, and in-process axum router
  Given registered agent `codex-a`
  When `POST /api/media/stage` receives base64 content with `from=codex-a`
  Then status is 200
  And the response attachment has `staged=true`, `size`, `name`, `mime`,
  `kind`, `source_path`, and a `path` inside the media directory
  When `GET /api/media/fetch?path=<attachment.path>` is requested
  Then status is 200
  And the response body bytes equal the original bytes
  And `Content-Type`, `Content-Length`, and inline `Content-Disposition` are set

Scenario: HTTP media stage and fetch reject invalid input
  Test:
    Package: agentd-surface
    Filter: http_media_stage_and_fetch_reject_invalid_input
  Level: HTTP integration
  Test Double: FakeRunHost, temp media directory, and in-process axum router
  Given no registered agent named `ghost`
  When `POST /api/media/stage` uses `from=ghost`
  Then status is 404
  When `POST /api/media/stage` omits `content_base64`
  Then status is 400
  When `GET /api/media/fetch?path=/tmp/agentd-p222-outside.txt` is requested
  Then status is 403
  When `GET /api/media/fetch?path=<missing path under media_dir>` is requested
  Then status is 404

Scenario: HTTP messages preserve staged attachment metadata
  Test:
    Package: agentd-surface
    Filter: http_messages_preserve_staged_attachment_metadata
  Level: HTTP integration
  Test Double: FakeRunHost, temp media directory, and in-process axum router
  Given registered agents `codex-a` and `codex-b`
  And a staged attachment returned by `/api/media/stage`
  When `POST /api/messages` sends a direct message to `codex-b` with that
  staged attachment
  Then `GET /api/inbox/codex-b` returns the same attachment with `staged=true`
  And the attachment `source_path`, `size`, `mime`, and `kind` are preserved

Scenario: production router reuses the media directory across rebuild
  Test:
    Package: agentd-bin
    Filter: daemon_router_media_stage_fetch_survives_router_rebuild
  Level: daemon HTTP integration
  Test Double: production host over temp SQLite store with fake backends
  Given a daemon router backed by a temp SQLite database
  And registered agent `codex-a`
  When `/api/media/stage` writes one attachment
  And the daemon router is rebuilt over the same database directory
  Then `/api/media/fetch` returns the original bytes from the staged path

Scenario: parity map records p222 media staging progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p222_media_stage_fetch_progress
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the agent-chat replacement parity map
  When the `attachments_media` row is inspected
  Then it mentions p222 `/api/media/stage` and `/api/media/fetch`
  And the row remains partial because Matrix media transfer, remote relay media,
  LocalPath localization, sanitization, dashboard previews, and import/cutover
  are not complete
