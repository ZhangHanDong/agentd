-- P225 / agent-chat replacement Phase H: non-destructive task/task-graph JSON import.

CREATE TABLE IF NOT EXISTS agent_chat_tasks (
    id TEXT PRIMARY KEY,
    title TEXT,
    description TEXT,
    status TEXT,
    priority TEXT,
    granularity TEXT,
    assignee TEXT,
    created_by TEXT,
    created_at TEXT,
    updated_at TEXT,
    started_at TEXT,
    completed_at TEXT,
    heartbeat_at TEXT,
    waiting_reason TEXT,
    waiting_until TEXT,
    parent_id TEXT,
    labels_json TEXT NOT NULL DEFAULT '[]',
    health_json TEXT,
    comments_json TEXT NOT NULL DEFAULT '[]',
    raw_json TEXT NOT NULL,
    imported_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_agent_chat_tasks_status
ON agent_chat_tasks(status);

CREATE INDEX IF NOT EXISTS idx_agent_chat_tasks_assignee
ON agent_chat_tasks(assignee);

CREATE TABLE IF NOT EXISTS agent_chat_task_graphs (
    id TEXT PRIMARY KEY,
    owner TEXT,
    label TEXT,
    status TEXT,
    raw_json TEXT NOT NULL,
    imported_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_agent_chat_task_graphs_status
ON agent_chat_task_graphs(status);

UPDATE schema_meta SET value = '8' WHERE key = 'version';
