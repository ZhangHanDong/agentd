-- agentd v1 schema. Local execution state only (design §3.3; Path B boundary).
-- Foreign keys are enforced at the connection level (pool.rs `.foreign_keys(true)`);
-- a PRAGMA here would be a no-op inside sqlx's migration transaction.
--
-- ── Δ: P0.1-trait ↔ P0.2-schema reconciliation ──────────────────────────────
-- The P0.1 engine-facing `Store` trait supplies a MINIMAL subset of what the
-- full §3.3 schema models (the engine has only run_id/workflow_sha/node_id/etc.
-- at execute time — no project, worktree, bundle, or agent registry yet). To let
-- `SqliteStore` satisfy that trait without fabricating junk, columns are sorted
-- into three buckets:
--   (1) store self-supplies  → kept NOT NULL (timestamps, status, attempt, JSON
--       blobs default to "{}"/"[]"). The store fills these from now()/computed.
--   (2) genuinely deferred    → nullable: review_runs.{task_run_id,bundle_path,
--       visibility,aggregator} (P1 ReviewBundle, D7); task_runs.{agent_id,
--       worktree_path,base_commit} (P0.3 TmuxBackend); human_waits.{interviewer,
--       options} (P0.6 Matrix).
--   (3) daemon-has-it/engine-doesn't → nullable + FK kept where it still holds:
--       runs.{project_id (FK kept; NULL doesn't violate),workflow_path}.
-- Added vs. the design sketch: review_runs.expected (only known at
-- insert_review_run time) and a dedicated `checkpoints` table (the trait's
-- write_checkpoint/load_checkpoint had no home). The `agents` FK on
-- review_verdicts.reviewer_id and task_runs.agent_id is DROPPED (kept as plain
-- TEXT): P0.1 has no agent registry to reference; re-add in P0.3.

-- ── projects ──
CREATE TABLE projects (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL UNIQUE,
    repo_path       TEXT NOT NULL,
    github_repo     TEXT,
    matrix_room_id  TEXT,
    mempal_wing     TEXT NOT NULL,
    agentflow_dir   TEXT NOT NULL DEFAULT '.agentflow',
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

-- ── agents ──
CREATE TABLE agents (
    id              TEXT PRIMARY KEY,
    mxid            TEXT NOT NULL UNIQUE,
    role            TEXT NOT NULL,
    backend         TEXT NOT NULL,
    backend_target  TEXT,
    prompt_profile  TEXT,
    enabled         INTEGER NOT NULL DEFAULT 1,
    created_at      INTEGER NOT NULL
);

CREATE TABLE project_agents (
    project_id      TEXT NOT NULL REFERENCES projects(id),
    agent_id        TEXT NOT NULL REFERENCES agents(id),
    invited_at      INTEGER NOT NULL,
    PRIMARY KEY (project_id, agent_id)
);

-- ── issues (boundary Δ3: per-run CACHE pulled from Specify, not authoritative) ──
CREATE TABLE issues (
    id              TEXT PRIMARY KEY,
    project_id      TEXT NOT NULL REFERENCES projects(id),
    github_number   INTEGER,
    title           TEXT NOT NULL,
    body            TEXT NOT NULL,
    labels          TEXT NOT NULL,
    state           TEXT NOT NULL,
    workflow_dot    TEXT,
    fetched_at      INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_issues_github ON issues(project_id, github_number) WHERE github_number IS NOT NULL;

-- ── runs ──
CREATE TABLE runs (
    id              TEXT PRIMARY KEY,
    project_id      TEXT REFERENCES projects(id),   -- Δ(3): NULL for engine-only runs; daemon supplies it
    issue_id        TEXT REFERENCES issues(id),
    workflow_path   TEXT,                            -- Δ(3): engine has workflow_sha, not the path
    workflow_sha    TEXT NOT NULL,
    status          TEXT NOT NULL,
    started_at      INTEGER NOT NULL,
    finished_at     INTEGER,
    last_heartbeat  INTEGER NOT NULL,
    current_node    TEXT
);
CREATE INDEX idx_runs_status_heartbeat ON runs(status, last_heartbeat);

-- ── node_outcomes (store supplies every column; PK gives attempt counting) ──
CREATE TABLE node_outcomes (
    run_id          TEXT NOT NULL REFERENCES runs(id),
    node_id         TEXT NOT NULL,
    attempt         INTEGER NOT NULL,
    status          TEXT NOT NULL,
    preferred_label TEXT,
    suggested_next  TEXT,
    context_delta   TEXT NOT NULL,
    artifacts       TEXT NOT NULL,
    started_at      INTEGER NOT NULL,
    finished_at     INTEGER NOT NULL,
    error_kind      TEXT,
    error_detail    TEXT,
    PRIMARY KEY (run_id, node_id, attempt)
);

-- ── checkpoints (NEW: 1:1 with agentd_core::engine::Checkpoint) ──
CREATE TABLE checkpoints (
    run_id           TEXT PRIMARY KEY REFERENCES runs(id),
    current_node     TEXT NOT NULL,
    completed_nodes  TEXT NOT NULL,   -- JSON array of node ids
    retry_counts     TEXT NOT NULL,   -- JSON object node_id -> u32
    context_snapshot TEXT NOT NULL,   -- JSON object (RunContext)
    workflow_sha     TEXT NOT NULL,
    updated_at       INTEGER NOT NULL
);

-- ── artifacts (content-addressed pointer store) ──
CREATE TABLE artifacts (
    sha256          TEXT PRIMARY KEY,
    kind            TEXT NOT NULL,
    path            TEXT NOT NULL,
    bytes           INTEGER NOT NULL,
    created_at      INTEGER NOT NULL,
    run_id          TEXT,
    node_id         TEXT
);

-- ── task_runs (codergen) ──
CREATE TABLE task_runs (
    id              TEXT PRIMARY KEY,
    run_id          TEXT NOT NULL REFERENCES runs(id),
    node_id         TEXT NOT NULL,
    agent_id        TEXT,            -- Δ(2): no agent registry FK in P0.1; re-add in P0.3
    worktree_path   TEXT,            -- Δ(2): P0.3 TmuxBackend
    base_commit     TEXT,            -- Δ(2): P0.3
    head_commit     TEXT,
    diff_sha256     TEXT REFERENCES artifacts(sha256),
    transcript_sha256 TEXT REFERENCES artifacts(sha256),
    status          TEXT NOT NULL,
    started_at      INTEGER NOT NULL,
    finished_at     INTEGER          -- complete_task_run sets this; NULL = still open (parks)
);

-- ── review_runs ──
CREATE TABLE review_runs (
    id              TEXT PRIMARY KEY,
    run_id          TEXT NOT NULL REFERENCES runs(id),
    node_id         TEXT NOT NULL,   -- the fan_out node that parked (lookup_park_by_review_run)
    expected        INTEGER NOT NULL,-- reviewer count this run waits for (review_expected)
    context_sha     TEXT NOT NULL,
    task_run_id     TEXT REFERENCES task_runs(id),  -- Δ(2): P1 ReviewBundle
    bundle_path     TEXT,            -- Δ(2): D7 — no disk bundle in P0.1
    visibility      TEXT,            -- Δ(2)
    aggregator      TEXT,            -- Δ(2)
    started_at      INTEGER NOT NULL,
    fan_in_at       INTEGER,
    verdict         TEXT
);

CREATE TABLE review_verdicts (
    review_run_id   TEXT NOT NULL REFERENCES review_runs(id),
    reviewer_id     TEXT NOT NULL,   -- Δ: no agents FK in P0.1; re-add in P0.3
    verdict         TEXT NOT NULL,
    findings        TEXT NOT NULL,   -- store supplies "" when P0.1 has none
    submitted_at    INTEGER NOT NULL,
    PRIMARY KEY (review_run_id, reviewer_id)  -- idempotent per reviewer
);

CREATE TABLE review_worktrees (
    review_run_id   TEXT NOT NULL REFERENCES review_runs(id),
    reviewer_id     TEXT NOT NULL,
    worktree_path   TEXT NOT NULL,
    released_at     INTEGER,
    PRIMARY KEY (review_run_id, reviewer_id)
);

-- ── human_waits ──
CREATE TABLE human_waits (
    id              TEXT PRIMARY KEY,
    run_id          TEXT NOT NULL REFERENCES runs(id),
    node_id         TEXT NOT NULL,
    interviewer     TEXT,            -- Δ(2): P0.6 Matrix
    prompt          TEXT NOT NULL,
    options         TEXT,            -- Δ(2): P0.6
    opened_at       INTEGER NOT NULL,
    timeout_at      INTEGER,
    answered_at     INTEGER,         -- NULL = open (parks)
    answer          TEXT,
    feedback        TEXT,
    answerer_mxid   TEXT
);
CREATE INDEX idx_human_waits_open ON human_waits(answered_at) WHERE answered_at IS NULL;

-- ── mempal_outbox ──
CREATE TABLE mempal_outbox (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id          TEXT NOT NULL REFERENCES runs(id),
    node_id         TEXT NOT NULL,
    kind            TEXT NOT NULL,
    payload         TEXT NOT NULL,
    enqueued_at     INTEGER NOT NULL,
    drained_at      INTEGER,
    attempts        INTEGER NOT NULL DEFAULT 0,
    last_error      TEXT
);
CREATE INDEX idx_outbox_pending ON mempal_outbox(drained_at, enqueued_at) WHERE drained_at IS NULL;

-- ── matrix_events ──
CREATE TABLE matrix_events (
    event_id        TEXT PRIMARY KEY,
    project_id      TEXT NOT NULL REFERENCES projects(id),
    room_id         TEXT NOT NULL,
    sender_mxid     TEXT NOT NULL,
    kind            TEXT NOT NULL,
    payload         TEXT NOT NULL,
    occurred_at     INTEGER NOT NULL,
    handled_run_id  TEXT REFERENCES runs(id),
    handled_node    TEXT
);

-- ── events (append-only broadcast log) ──
CREATE TABLE events (
    seq             INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id      TEXT REFERENCES projects(id),
    run_id          TEXT REFERENCES runs(id),
    kind            TEXT NOT NULL,
    payload         TEXT NOT NULL,
    emitted_at      INTEGER NOT NULL
);
CREATE INDEX idx_events_run_seq ON events(run_id, seq);

-- ── schema metadata ──
CREATE TABLE schema_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT INTO schema_meta (key, value) VALUES ('version', '1');
