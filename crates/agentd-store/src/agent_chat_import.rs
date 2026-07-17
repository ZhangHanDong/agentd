//! Non-destructive agent-chat agent JSON import and shadow-audit helpers.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use crate::StoreError;
use crate::agent_repo::{self, OfflineAgent, RegisterAgent};
use crate::message_repo::{self, DirectMessageInput, GroupCreateInput, GroupMessageInput};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalLegacyRecord {
    pub surface: agentd_core::ports::CutoverSurface,
    pub legacy_id: String,
    pub native_id: String,
    pub record_sha256: String,
    pub decision_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentChatCanonicalSnapshot {
    pub source_sha256: String,
    pub file_count: u32,
    pub record_count: u64,
    pub unsupported_count: u64,
    pub records: Vec<CanonicalLegacyRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentChatImportMode {
    DryRun,
    Execute,
}

impl AgentChatImportMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::DryRun => "dry-run",
            Self::Execute => "execute",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentChatImportOptions {
    pub mode: AgentChatImportMode,
}

impl Default for AgentChatImportOptions {
    fn default() -> Self {
        Self {
            mode: AgentChatImportMode::DryRun,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SurfaceCount {
    pub source: usize,
    pub planned: usize,
    pub imported: usize,
    pub skipped: usize,
    pub missing: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageSurfaceCount {
    pub source: usize,
    pub planned: usize,
    pub imported: usize,
    pub skipped: usize,
    pub missing: usize,
    pub direct: usize,
    pub group: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentChatAgentImportReport {
    pub mode: String,
    pub ok: bool,
    pub agents: SurfaceCount,
    pub warnings: Vec<String>,
    pub drift: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentChatMessageImportOptions {
    pub mode: AgentChatImportMode,
}

impl Default for AgentChatMessageImportOptions {
    fn default() -> Self {
        Self {
            mode: AgentChatImportMode::DryRun,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentChatMessageImportReport {
    pub mode: String,
    pub ok: bool,
    pub groups: SurfaceCount,
    pub messages: MessageSurfaceCount,
    pub cursors: SurfaceCount,
    pub warnings: Vec<String>,
    pub drift: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentChatTaskImportOptions {
    pub mode: AgentChatImportMode,
}

impl Default for AgentChatTaskImportOptions {
    fn default() -> Self {
        Self {
            mode: AgentChatImportMode::DryRun,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentChatTaskImportReport {
    pub mode: String,
    pub ok: bool,
    pub tasks: SurfaceCount,
    pub task_graphs: SurfaceCount,
    pub warnings: Vec<String>,
    pub drift: Vec<String>,
}

pub fn plan_agents_from_agent_chat(
    agent_chat: &Path,
) -> Result<AgentChatAgentImportReport, StoreError> {
    let source = SourceSnapshot::read(agent_chat)?;
    Ok(source.report(AgentChatImportMode::DryRun.as_str()))
}

pub async fn import_agents_from_agent_chat(
    pool: &SqlitePool,
    agent_chat: &Path,
    options: AgentChatImportOptions,
) -> Result<AgentChatAgentImportReport, StoreError> {
    let source = SourceSnapshot::read(agent_chat)?;
    let mut report = source.report(options.mode.as_str());
    if options.mode == AgentChatImportMode::DryRun {
        return Ok(report);
    }

    for agent in &source.agents {
        import_agent(pool, agent).await?;
    }
    report.agents.imported = report.agents.planned;
    Ok(report)
}

pub fn plan_messages_from_agent_chat(
    agent_chat: &Path,
) -> Result<AgentChatMessageImportReport, StoreError> {
    let source = MessageSourceSnapshot::read(agent_chat)?;
    Ok(source.report(AgentChatImportMode::DryRun.as_str()))
}

pub async fn import_messages_from_agent_chat(
    pool: &SqlitePool,
    agent_chat: &Path,
    options: AgentChatMessageImportOptions,
) -> Result<AgentChatMessageImportReport, StoreError> {
    let source = MessageSourceSnapshot::read(agent_chat)?;
    let mut report = source.report(options.mode.as_str());
    if options.mode == AgentChatImportMode::DryRun {
        return Ok(report);
    }

    for group in source.groups.values() {
        ensure_group(pool, group).await?;
    }
    for message in &source.messages {
        if let ImportMessage::Group(input) = message
            && !source.groups.contains_key(&input.group)
        {
            ensure_group(
                pool,
                &ImportGroup {
                    name: input.group.clone(),
                    members: Vec::new(),
                    created_at: None,
                },
            )
            .await?;
        }
    }
    for message in &source.messages {
        match message {
            ImportMessage::Direct(input) => {
                message_repo::insert_direct_message(pool, input.clone()).await?;
            }
            ImportMessage::Group(input) => {
                message_repo::insert_group_message(pool, input.clone()).await?;
            }
        }
    }
    apply_imported_cursors(pool, &source.cursors).await?;
    report.groups.imported = report.groups.planned;
    report.messages.imported = report.messages.planned;
    report.cursors.imported = report.cursors.planned;
    Ok(report)
}

pub async fn shadow_messages(
    pool: &SqlitePool,
    agent_chat: &Path,
) -> Result<AgentChatMessageImportReport, StoreError> {
    let source = MessageSourceSnapshot::read(agent_chat)?;
    let mut report = source.report("shadow-audit");
    let direct_ids = existing_ids(pool, "direct_messages").await?;
    let group_ids = existing_ids(pool, "group_messages").await?;

    for message in &source.messages {
        match message {
            ImportMessage::Direct(input) => {
                let Some(id) = input.message_id.as_ref() else {
                    continue;
                };
                if !direct_ids.contains(id) {
                    report.messages.missing += 1;
                    report.drift.push(format!("missing direct message: {id}"));
                }
            }
            ImportMessage::Group(input) => {
                let Some(id) = input.message_id.as_ref() else {
                    continue;
                };
                if !group_ids.contains(id) {
                    report.messages.missing += 1;
                    report.drift.push(format!("missing group message: {id}"));
                }
            }
        }
    }
    report.ok = report.drift.is_empty();
    Ok(report)
}

pub fn plan_tasks_from_agent_chat(
    agent_chat: &Path,
) -> Result<AgentChatTaskImportReport, StoreError> {
    let source = TaskSourceSnapshot::read(agent_chat)?;
    Ok(source.report(AgentChatImportMode::DryRun.as_str()))
}

pub async fn import_tasks_from_agent_chat(
    pool: &SqlitePool,
    agent_chat: &Path,
    options: AgentChatTaskImportOptions,
) -> Result<AgentChatTaskImportReport, StoreError> {
    let source = TaskSourceSnapshot::read(agent_chat)?;
    let mut report = source.report(options.mode.as_str());
    if options.mode == AgentChatImportMode::DryRun {
        return Ok(report);
    }

    let imported_at = now_unix_for_import();
    let mut tx = pool.begin().await?;
    for task in &source.tasks {
        upsert_imported_task(&mut tx, task, imported_at).await?;
    }
    for graph in &source.task_graphs {
        upsert_imported_task_graph(&mut tx, graph, imported_at).await?;
    }
    tx.commit().await?;
    report.tasks.imported = report.tasks.planned;
    report.task_graphs.imported = report.task_graphs.planned;
    Ok(report)
}

pub async fn shadow_tasks(
    pool: &SqlitePool,
    agent_chat: &Path,
) -> Result<AgentChatTaskImportReport, StoreError> {
    let source = TaskSourceSnapshot::read(agent_chat)?;
    let mut report = source.report("shadow-audit");
    let task_ids = existing_ids(pool, "agent_chat_tasks").await?;
    let graph_ids = existing_ids(pool, "agent_chat_task_graphs").await?;

    for task in &source.tasks {
        if !task_ids.contains(&task.id) {
            report.tasks.missing += 1;
            report.drift.push(format!("missing task: {}", task.id));
        }
    }
    for graph in &source.task_graphs {
        if !graph_ids.contains(&graph.id) {
            report.task_graphs.missing += 1;
            report
                .drift
                .push(format!("missing task graph: {}", graph.id));
        }
    }
    report.ok = report.drift.is_empty();
    Ok(report)
}

pub async fn shadow_agents(
    pool: &SqlitePool,
    agent_chat: &Path,
) -> Result<AgentChatAgentImportReport, StoreError> {
    let source = SourceSnapshot::read(agent_chat)?;
    let mut report = source.report("shadow-audit");
    let existing = agent_repo::list_agents(pool)
        .await?
        .into_iter()
        .map(|agent| agent.name)
        .collect::<BTreeSet<_>>();

    for agent in &source.agents {
        if !existing.contains(&agent.name) {
            report.agents.missing += 1;
            report.drift.push(format!("missing agent: {}", agent.name));
        }
    }
    report.ok = report.drift.is_empty();
    Ok(report)
}

#[derive(Debug)]
struct SourceSnapshot {
    agents: Vec<ImportAgent>,
    skipped_agents: usize,
}

impl SourceSnapshot {
    fn read(agent_chat: &Path) -> Result<Self, StoreError> {
        let path = agent_chat.join("data/agents.json");
        let value = read_json_or_default(&path, Value::Object(Map::default()))?;
        let (agents, skipped_agents) = parse_agents(&value)?;
        Ok(Self {
            agents,
            skipped_agents,
        })
    }

    fn report(&self, mode: &str) -> AgentChatAgentImportReport {
        let mut warnings = Vec::new();
        if self.skipped_agents > 0 {
            warnings.push(format!(
                "skipped {} unsupported agent row(s)",
                self.skipped_agents
            ));
        }
        AgentChatAgentImportReport {
            mode: mode.to_string(),
            ok: true,
            agents: SurfaceCount {
                source: self.agents.len() + self.skipped_agents,
                planned: self.agents.len(),
                skipped: self.skipped_agents,
                ..SurfaceCount::default()
            },
            warnings,
            drift: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct ImportAgent {
    name: String,
    role: Option<String>,
    capability: Option<String>,
    runtime: Option<String>,
    model: Option<String>,
    tmux_target: Option<String>,
    home_dir: Option<String>,
    workdir: Option<String>,
    state_dir: Option<String>,
    server: Option<String>,
    runtime_profile: Value,
    online: Option<bool>,
    offline_reason: Option<String>,
}

#[derive(Debug, Clone)]
struct ImportGroup {
    name: String,
    members: Vec<String>,
    created_at: Option<i64>,
}

#[derive(Debug, Clone)]
enum ImportMessage {
    Direct(DirectMessageInput),
    Group(GroupMessageInput),
}

#[derive(Debug, Clone)]
struct ImportCursor {
    agent: String,
    inbox_ts: Option<i64>,
    inbox_id: Option<String>,
    group_cursors: Vec<ImportGroupCursor>,
}

#[derive(Debug, Clone)]
struct ImportGroupCursor {
    group: String,
    ts: i64,
    id: Option<String>,
}

#[derive(Debug)]
struct MessageSourceSnapshot {
    groups: BTreeMap<String, ImportGroup>,
    messages: Vec<ImportMessage>,
    cursors: Vec<ImportCursor>,
    source_messages: usize,
    skipped_messages: usize,
    skipped_groups: usize,
}

impl MessageSourceSnapshot {
    fn read(agent_chat: &Path) -> Result<Self, StoreError> {
        let groups_value = read_json_or_default(
            &agent_chat.join("data/groups.json"),
            Value::Object(Map::default()),
        )?;
        let messages_value = read_json_or_default(
            &agent_chat.join("data/messages.json"),
            Value::Array(Vec::new()),
        )?;
        let cursors_value = read_json_or_default(
            &agent_chat.join("data/cursors.json"),
            Value::Object(Map::default()),
        )?;
        let (groups, skipped_groups) = parse_groups(&groups_value)?;
        let (messages, source_messages, skipped_messages) = parse_messages(&messages_value)?;
        let cursors = parse_cursors(&cursors_value)?;
        Ok(Self {
            groups,
            messages,
            cursors,
            source_messages,
            skipped_messages,
            skipped_groups,
        })
    }

    fn report(&self, mode: &str) -> AgentChatMessageImportReport {
        let mut warnings = Vec::new();
        if self.skipped_groups > 0 {
            warnings.push(format!(
                "skipped {} unsupported group row(s)",
                self.skipped_groups
            ));
        }
        if self.skipped_messages > 0 {
            warnings.push(format!(
                "skipped {} unsupported message row(s)",
                self.skipped_messages
            ));
        }
        let direct = self
            .messages
            .iter()
            .filter(|message| matches!(message, ImportMessage::Direct(_)))
            .count();
        let group = self.messages.len().saturating_sub(direct);
        AgentChatMessageImportReport {
            mode: mode.to_string(),
            ok: true,
            groups: SurfaceCount {
                source: self.groups.len() + self.skipped_groups,
                planned: self.groups.len(),
                skipped: self.skipped_groups,
                ..SurfaceCount::default()
            },
            messages: MessageSurfaceCount {
                source: self.source_messages,
                planned: self.messages.len(),
                skipped: self.skipped_messages,
                direct,
                group,
                ..MessageSurfaceCount::default()
            },
            cursors: SurfaceCount {
                source: self.cursors.len(),
                planned: self.cursors.len(),
                ..SurfaceCount::default()
            },
            warnings,
            drift: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct ImportTask {
    id: String,
    title: Option<String>,
    description: Option<String>,
    status: Option<String>,
    priority: Option<String>,
    granularity: Option<String>,
    assignee: Option<String>,
    created_by: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    heartbeat_at: Option<String>,
    waiting_reason: Option<String>,
    waiting_until: Option<String>,
    parent_id: Option<String>,
    labels_json: String,
    health_json: Option<String>,
    comments_json: String,
    raw_json: String,
}

#[derive(Debug, Clone)]
struct ImportTaskGraph {
    id: String,
    owner: Option<String>,
    label: Option<String>,
    status: Option<String>,
    raw_json: String,
}

#[derive(Debug)]
struct TaskSourceSnapshot {
    tasks: Vec<ImportTask>,
    task_graphs: Vec<ImportTaskGraph>,
    source_tasks: usize,
    source_task_graphs: usize,
    skipped_tasks: usize,
    skipped_task_graphs: usize,
}

impl TaskSourceSnapshot {
    fn read(agent_chat: &Path) -> Result<Self, StoreError> {
        let tasks_value = read_json_or_default(
            &agent_chat.join("data/tasks.json"),
            Value::Array(Vec::new()),
        )?;
        let task_graphs_value = read_json_or_default(
            &agent_chat.join("data/task_graphs.json"),
            Value::Object(Map::default()),
        )?;
        let (tasks, source_tasks, skipped_tasks) = parse_tasks(&tasks_value)?;
        let (task_graphs, source_task_graphs, skipped_task_graphs) =
            parse_task_graphs(&task_graphs_value)?;
        Ok(Self {
            tasks,
            task_graphs,
            source_tasks,
            source_task_graphs,
            skipped_tasks,
            skipped_task_graphs,
        })
    }

    fn report(&self, mode: &str) -> AgentChatTaskImportReport {
        let mut warnings = Vec::new();
        if self.skipped_tasks > 0 {
            warnings.push(format!(
                "skipped {} unsupported task row(s)",
                self.skipped_tasks
            ));
        }
        if self.skipped_task_graphs > 0 {
            warnings.push(format!(
                "skipped {} unsupported task graph row(s)",
                self.skipped_task_graphs
            ));
        }
        AgentChatTaskImportReport {
            mode: mode.to_string(),
            ok: true,
            tasks: SurfaceCount {
                source: self.source_tasks,
                planned: self.tasks.len(),
                skipped: self.skipped_tasks,
                ..SurfaceCount::default()
            },
            task_graphs: SurfaceCount {
                source: self.source_task_graphs,
                planned: self.task_graphs.len(),
                skipped: self.skipped_task_graphs,
                ..SurfaceCount::default()
            },
            warnings,
            drift: Vec::new(),
        }
    }
}

async fn upsert_imported_task(
    tx: &mut Transaction<'_, Sqlite>,
    task: &ImportTask,
    imported_at: i64,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO agent_chat_tasks \
         (id, title, description, status, priority, granularity, assignee, created_by, \
          created_at, updated_at, started_at, completed_at, heartbeat_at, waiting_reason, \
          waiting_until, parent_id, labels_json, health_json, comments_json, raw_json, imported_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
          title = excluded.title, \
          description = excluded.description, \
          status = excluded.status, \
          priority = excluded.priority, \
          granularity = excluded.granularity, \
          assignee = excluded.assignee, \
          created_by = excluded.created_by, \
          created_at = excluded.created_at, \
          updated_at = excluded.updated_at, \
          started_at = excluded.started_at, \
          completed_at = excluded.completed_at, \
          heartbeat_at = excluded.heartbeat_at, \
          waiting_reason = excluded.waiting_reason, \
          waiting_until = excluded.waiting_until, \
          parent_id = excluded.parent_id, \
          labels_json = excluded.labels_json, \
          health_json = excluded.health_json, \
          comments_json = excluded.comments_json, \
          raw_json = excluded.raw_json, \
          imported_at = excluded.imported_at",
    )
    .bind(&task.id)
    .bind(task.title.as_deref())
    .bind(task.description.as_deref())
    .bind(task.status.as_deref())
    .bind(task.priority.as_deref())
    .bind(task.granularity.as_deref())
    .bind(task.assignee.as_deref())
    .bind(task.created_by.as_deref())
    .bind(task.created_at.as_deref())
    .bind(task.updated_at.as_deref())
    .bind(task.started_at.as_deref())
    .bind(task.completed_at.as_deref())
    .bind(task.heartbeat_at.as_deref())
    .bind(task.waiting_reason.as_deref())
    .bind(task.waiting_until.as_deref())
    .bind(task.parent_id.as_deref())
    .bind(&task.labels_json)
    .bind(task.health_json.as_deref())
    .bind(&task.comments_json)
    .bind(&task.raw_json)
    .bind(imported_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn upsert_imported_task_graph(
    tx: &mut Transaction<'_, Sqlite>,
    graph: &ImportTaskGraph,
    imported_at: i64,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO agent_chat_task_graphs \
         (id, owner, label, status, raw_json, imported_at) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
          owner = excluded.owner, \
          label = excluded.label, \
          status = excluded.status, \
          raw_json = excluded.raw_json, \
          imported_at = excluded.imported_at",
    )
    .bind(&graph.id)
    .bind(graph.owner.as_deref())
    .bind(graph.label.as_deref())
    .bind(graph.status.as_deref())
    .bind(&graph.raw_json)
    .bind(imported_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn import_agent(pool: &SqlitePool, agent: &ImportAgent) -> Result<(), StoreError> {
    agent_repo::register_agent(
        pool,
        RegisterAgent {
            name: agent.name.clone(),
            role: agent.role.clone(),
            capability: agent.capability.clone(),
            runtime: agent.runtime.clone(),
            model: agent.model.clone(),
            native_runtime_ref: None,
            home_dir: agent.home_dir.clone(),
            workdir: agent.workdir.clone(),
            state_dir: agent.state_dir.clone(),
            server: agent.server.clone(),
            runtime_profile: agent.runtime_profile.clone(),
        },
    )
    .await?;

    if agent.online == Some(false) {
        agent_repo::mark_agent_offline(
            pool,
            &agent.name,
            OfflineAgent {
                reason: agent
                    .offline_reason
                    .clone()
                    .or_else(|| Some("agent-chat-offline".to_string())),
                clear_runtime: false,
            },
        )
        .await?;
    }
    Ok(())
}

async fn ensure_group(pool: &SqlitePool, group: &ImportGroup) -> Result<(), StoreError> {
    if message_repo::get_group(pool, &group.name).await?.is_some() {
        message_repo::update_group_members(pool, &group.name, &group.members, &[]).await?;
        return Ok(());
    }
    message_repo::create_group(
        pool,
        GroupCreateInput {
            name: group.name.clone(),
            members: group.members.clone(),
        },
    )
    .await?;
    if let Some(created_at) = group.created_at {
        sqlx::query("UPDATE groups SET created_at = ? WHERE name = ?")
            .bind(created_at)
            .bind(&group.name)
            .execute(pool)
            .await?;
    }
    Ok(())
}

async fn apply_imported_cursors(
    pool: &SqlitePool,
    cursors: &[ImportCursor],
) -> Result<(), StoreError> {
    let read_at = now_unix_for_import();
    for cursor in cursors {
        if let Some(inbox_ts) = cursor.inbox_ts {
            mark_direct_inbox_read(
                pool,
                &cursor.agent,
                inbox_ts,
                cursor.inbox_id.as_deref(),
                read_at,
            )
            .await?;
            mark_group_mentions_read(
                pool,
                &cursor.agent,
                inbox_ts,
                cursor.inbox_id.as_deref(),
                read_at,
            )
            .await?;
        }
        for group_cursor in &cursor.group_cursors {
            mark_group_history_read(pool, &cursor.agent, group_cursor, read_at).await?;
        }
    }
    Ok(())
}

async fn mark_direct_inbox_read(
    pool: &SqlitePool,
    agent: &str,
    ts: i64,
    id: Option<&str>,
    read_at: i64,
) -> Result<(), StoreError> {
    if let Some(id) = id {
        sqlx::query(
            "UPDATE direct_messages SET read_at = ? \
             WHERE to_agent = ? AND read_at IS NULL AND (ts < ? OR (ts = ? AND id <= ?))",
        )
        .bind(read_at)
        .bind(agent)
        .bind(ts)
        .bind(ts)
        .bind(id)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "UPDATE direct_messages SET read_at = ? \
             WHERE to_agent = ? AND read_at IS NULL AND ts <= ?",
        )
        .bind(read_at)
        .bind(agent)
        .bind(ts)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn mark_group_mentions_read(
    pool: &SqlitePool,
    agent: &str,
    ts: i64,
    id: Option<&str>,
    read_at: i64,
) -> Result<(), StoreError> {
    let rows = if let Some(id) = id {
        sqlx::query(
            "SELECT id, mentions_json FROM group_messages \
             WHERE ts < ? OR (ts = ? AND id <= ?)",
        )
        .bind(ts)
        .bind(ts)
        .bind(id)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query("SELECT id, mentions_json FROM group_messages WHERE ts <= ?")
            .bind(ts)
            .fetch_all(pool)
            .await?
    };
    for row in rows {
        let message_id: String = row.get("id");
        let mentions_json: String = row.get("mentions_json");
        let mentions: Vec<String> = serde_json::from_str(&mentions_json)?;
        if mentions
            .iter()
            .any(|mention| mention.eq_ignore_ascii_case(agent))
        {
            sqlx::query(
                "INSERT OR IGNORE INTO group_mention_reads (agent_name, message_id, read_at) \
                 VALUES (?, ?, ?)",
            )
            .bind(agent)
            .bind(message_id)
            .bind(read_at)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

async fn mark_group_history_read(
    pool: &SqlitePool,
    agent: &str,
    cursor: &ImportGroupCursor,
    read_at: i64,
) -> Result<(), StoreError> {
    let rows = if let Some(id) = cursor.id.as_deref() {
        sqlx::query(
            "SELECT id FROM group_messages \
             WHERE group_name = ? AND (ts < ? OR (ts = ? AND id <= ?))",
        )
        .bind(&cursor.group)
        .bind(cursor.ts)
        .bind(cursor.ts)
        .bind(id)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query("SELECT id FROM group_messages WHERE group_name = ? AND ts <= ?")
            .bind(&cursor.group)
            .bind(cursor.ts)
            .fetch_all(pool)
            .await?
    };
    for row in rows {
        let message_id: String = row.get("id");
        sqlx::query(
            "INSERT OR IGNORE INTO group_message_reads \
             (agent_name, group_name, message_id, read_at) VALUES (?, ?, ?, ?)",
        )
        .bind(agent)
        .bind(&cursor.group)
        .bind(message_id)
        .bind(read_at)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn existing_ids(pool: &SqlitePool, table: &str) -> Result<BTreeSet<String>, StoreError> {
    let sql = match table {
        "direct_messages" => "SELECT id FROM direct_messages",
        "group_messages" => "SELECT id FROM group_messages",
        "agent_chat_tasks" => "SELECT id FROM agent_chat_tasks",
        "agent_chat_task_graphs" => "SELECT id FROM agent_chat_task_graphs",
        _ => return Err(StoreError::Invariant(format!("unsupported table: {table}"))),
    };
    let rows = sqlx::query(sql).fetch_all(pool).await?;
    Ok(rows.iter().map(|row| row.get::<String, _>("id")).collect())
}

fn read_json_or_default(path: &Path, default: Value) -> Result<Value, StoreError> {
    if !path.exists() {
        return Ok(default);
    }
    let bytes = std::fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn parse_agents(value: &Value) -> Result<(Vec<ImportAgent>, usize), StoreError> {
    let object = value
        .as_object()
        .ok_or_else(|| StoreError::Invariant("agents.json must be a JSON object".to_string()))?;
    let mut agents = Vec::new();
    let mut skipped = 0;
    for (key, value) in object {
        let Some(agent) = parse_agent(key, value) else {
            skipped += 1;
            continue;
        };
        agents.push(agent);
    }
    Ok((agents, skipped))
}

fn parse_groups(value: &Value) -> Result<(BTreeMap<String, ImportGroup>, usize), StoreError> {
    let object = value
        .as_object()
        .ok_or_else(|| StoreError::Invariant("groups.json must be a JSON object".to_string()))?;
    let mut groups = BTreeMap::new();
    let mut skipped = 0;
    for (key, value) in object {
        let Some(group) = parse_group(key, value)? else {
            skipped += 1;
            continue;
        };
        groups.insert(group.name.clone(), group);
    }
    Ok((groups, skipped))
}

fn parse_group(key: &str, value: &Value) -> Result<Option<ImportGroup>, StoreError> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let name = string_field(object, "name").unwrap_or_else(|| key.to_string());
    if name.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(ImportGroup {
        name,
        members: string_array_field(object, "members")?.unwrap_or_default(),
        created_at: i64_field(object, "createdAt"),
    }))
}

fn parse_messages(value: &Value) -> Result<(Vec<ImportMessage>, usize, usize), StoreError> {
    let array = value
        .as_array()
        .ok_or_else(|| StoreError::Invariant("messages.json must be a JSON array".to_string()))?;
    let mut messages = Vec::new();
    let mut skipped = 0;
    for value in array {
        let Some(message) = parse_message(value)? else {
            skipped += 1;
            continue;
        };
        messages.push(message);
    }
    Ok((messages, array.len(), skipped))
}

fn parse_message(value: &Value) -> Result<Option<ImportMessage>, StoreError> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let id = string_field(object, "id");
    let from = string_field(object, "from");
    let to = string_field(object, "to");
    let group = string_field(object, "group");
    if to.is_some() == group.is_some() {
        return Ok(None);
    }
    let message_id = id.ok_or_else(|| StoreError::Invariant("message id required".to_string()))?;
    let from = from.unwrap_or_else(|| "system".to_string());
    let summary = string_field(object, "summary")
        .ok_or_else(|| StoreError::Invariant("message summary required".to_string()))?;
    let full = string_field(object, "full").unwrap_or_default();
    let message_type = string_field(object, "type");
    let priority = string_field(object, "priority");
    let reply_to = string_field(object, "reply_to");
    let source = string_field(object, "source");
    let attachments = value_array_field(object, "attachments")?.unwrap_or_default();
    if let Some(to) = to {
        return Ok(Some(ImportMessage::Direct(DirectMessageInput {
            message_id: Some(message_id),
            ts: i64_field(object, "ts"),
            from,
            to,
            message_type,
            priority,
            summary,
            full,
            reply_to,
            source,
            source_room: string_field(object, "sourceRoom"),
            sender_mxid: string_field(object, "senderMxid"),
            trust_level: string_field(object, "trustLevel"),
            from_id: string_field(object, "fromId"),
            schema: optional_object_field(object, "schema")?,
            attachments,
        })));
    }

    let Some(group) = group else {
        return Ok(None);
    };
    Ok(Some(ImportMessage::Group(GroupMessageInput {
        message_id: Some(message_id),
        ts: i64_field(object, "ts"),
        from,
        group,
        message_type,
        priority,
        summary,
        full,
        mentions: string_array_field(object, "mentions")?.unwrap_or_default(),
        reply_to,
        source,
        schema: optional_object_field(object, "schema")?,
        attachments,
    })))
}

fn parse_tasks(value: &Value) -> Result<(Vec<ImportTask>, usize, usize), StoreError> {
    let array = value
        .as_array()
        .ok_or_else(|| StoreError::Invariant("tasks.json must be a JSON array".to_string()))?;
    let mut tasks = Vec::new();
    let mut skipped = 0;
    for value in array {
        let Some(task) = parse_task(value)? else {
            skipped += 1;
            continue;
        };
        tasks.push(task);
    }
    Ok((tasks, array.len(), skipped))
}

fn parse_task(value: &Value) -> Result<Option<ImportTask>, StoreError> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let Some(id) = string_field(object, "id") else {
        return Ok(None);
    };
    Ok(Some(ImportTask {
        id,
        title: string_field(object, "title"),
        description: string_field(object, "description"),
        status: string_field(object, "status"),
        priority: string_field(object, "priority"),
        granularity: string_field(object, "granularity"),
        assignee: string_field(object, "assignee"),
        created_by: string_field(object, "created_by"),
        created_at: string_field(object, "created_at"),
        updated_at: string_field(object, "updated_at"),
        started_at: string_field(object, "started_at"),
        completed_at: string_field(object, "completed_at"),
        heartbeat_at: string_field(object, "heartbeat_at"),
        waiting_reason: string_field(object, "waiting_reason"),
        waiting_until: string_field(object, "waiting_until"),
        parent_id: string_field(object, "parent_id"),
        labels_json: json_field_or_default(object, "labels", "[]")?,
        health_json: json_field(object, "health")?,
        comments_json: json_field_or_default(object, "comments", "[]")?,
        raw_json: serde_json::to_string(value)?,
    }))
}

fn parse_task_graphs(value: &Value) -> Result<(Vec<ImportTaskGraph>, usize, usize), StoreError> {
    let object = value.as_object().ok_or_else(|| {
        StoreError::Invariant("task_graphs.json must be a JSON object".to_string())
    })?;
    let mut graphs = Vec::new();
    let mut skipped = 0;
    for (key, value) in object {
        let Some(graph) = parse_task_graph(key, value)? else {
            skipped += 1;
            continue;
        };
        graphs.push(graph);
    }
    Ok((graphs, object.len(), skipped))
}

fn parse_task_graph(key: &str, value: &Value) -> Result<Option<ImportTaskGraph>, StoreError> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let id = string_field(object, "id").unwrap_or_else(|| key.to_string());
    if id.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(ImportTaskGraph {
        id,
        owner: string_field(object, "owner"),
        label: string_field(object, "label"),
        status: string_field(object, "status"),
        raw_json: serde_json::to_string(value)?,
    }))
}

fn parse_cursors(value: &Value) -> Result<Vec<ImportCursor>, StoreError> {
    let object = value
        .as_object()
        .ok_or_else(|| StoreError::Invariant("cursors.json must be a JSON object".to_string()))?;
    let mut cursors = Vec::new();
    for (agent, value) in object {
        let Some(cursor) = parse_cursor(agent, value) else {
            continue;
        };
        cursors.push(cursor);
    }
    Ok(cursors)
}

fn parse_cursor(agent: &str, value: &Value) -> Option<ImportCursor> {
    let object = value.as_object()?;
    let agent = agent.trim();
    if agent.is_empty() {
        return None;
    }
    let groups = object
        .get("groups")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let group_ids = object
        .get("groupIds")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut group_cursors = Vec::new();
    for (group, ts_value) in groups {
        let Some(ts) = value_as_i64(&ts_value) else {
            continue;
        };
        group_cursors.push(ImportGroupCursor {
            id: group_ids
                .get(&group)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            group,
            ts,
        });
    }
    Some(ImportCursor {
        agent: agent.to_string(),
        inbox_ts: object.get("inbox").and_then(value_as_i64),
        inbox_id: string_field(object, "inboxId"),
        group_cursors,
    })
}

fn parse_agent(key: &str, value: &Value) -> Option<ImportAgent> {
    let object = value.as_object()?;
    let name = string_field(object, "name").unwrap_or_else(|| key.to_string());
    if name.trim().is_empty() {
        return None;
    }
    let agent_id = string_field(object, "agentId");
    let runtime_profile = runtime_profile(object.get("runtimeProfile"), agent_id.as_deref());
    Some(ImportAgent {
        name,
        role: string_field(object, "role"),
        capability: string_field(object, "capability"),
        runtime: string_field(object, "type").or_else(|| string_field(object, "runtime")),
        model: string_field(object, "agentModelVersion").or_else(|| string_field(object, "model")),
        tmux_target: string_field(object, "tmux"),
        home_dir: string_field(object, "homeDir"),
        workdir: string_field(object, "workdir"),
        state_dir: string_field(object, "stateDir"),
        server: string_field(object, "server"),
        runtime_profile,
        online: object.get("online").and_then(Value::as_bool),
        offline_reason: string_field(object, "offlineReason"),
    })
}

fn string_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn string_array_field(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<Vec<String>>, StoreError> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    let Some(array) = value.as_array() else {
        return Err(StoreError::Invariant(format!("{key} must be an array")));
    };
    Ok(Some(
        array
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect(),
    ))
}

fn value_array_field(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<Vec<Value>>, StoreError> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    let Some(array) = value.as_array() else {
        return Err(StoreError::Invariant(format!("{key} must be an array")));
    };
    Ok(Some(array.clone()))
}

fn json_field(object: &Map<String, Value>, key: &str) -> Result<Option<String>, StoreError> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(serde_json::to_string(value)?))
}

fn json_field_or_default(
    object: &Map<String, Value>,
    key: &str,
    default: &str,
) -> Result<String, StoreError> {
    match json_field(object, key)? {
        Some(value) => Ok(value),
        None => Ok(default.to_string()),
    }
}

fn optional_object_field(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<Value>, StoreError> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    if value.is_object() {
        return Ok(Some(value.clone()));
    }
    Err(StoreError::Invariant(format!("{key} must be an object")))
}

fn i64_field(object: &Map<String, Value>, key: &str) -> Option<i64> {
    object.get(key).and_then(value_as_i64)
}

fn value_as_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
}

fn runtime_profile(value: Option<&Value>, agent_id: Option<&str>) -> Value {
    let mut object = match value {
        Some(Value::Object(map)) => map.clone(),
        Some(Value::String(profile)) => {
            let mut map = Map::new();
            map.insert("agentChatRuntimeProfile".to_string(), json!(profile));
            map
        }
        _ => Map::new(),
    };
    if let Some(agent_id) = agent_id {
        object.insert("agentChatAgentId".to_string(), json!(agent_id));
    }
    Value::Object(object)
}

fn now_unix_for_import() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

#[allow(clippy::too_many_lines)]
pub fn canonical_agent_chat_snapshot(
    agent_chat: &Path,
) -> Result<AgentChatCanonicalSnapshot, StoreError> {
    let files = [
        ("agents.json", Value::Object(Map::new())),
        ("groups.json", Value::Object(Map::new())),
        ("messages.json", Value::Array(Vec::new())),
        ("cursors.json", Value::Object(Map::new())),
        ("tasks.json", Value::Array(Vec::new())),
        ("task_graphs.json", Value::Object(Map::new())),
    ];
    let mut source_hasher = Sha256::new();
    for (name, default) in &files {
        let value = read_json_or_default(&agent_chat.join("data").join(name), default.clone())?;
        hash_snapshot_field(&mut source_hasher, name.as_bytes());
        hash_snapshot_field(&mut source_hasher, &canonical_json_bytes(&value)?);
    }

    let agents = SourceSnapshot::read(agent_chat)?;
    let messaging = MessageSourceSnapshot::read(agent_chat)?;
    let tasks = TaskSourceSnapshot::read(agent_chat)?;
    let mut records = Vec::new();
    for agent in agents.agents {
        let status = if agent.online == Some(false) {
            "offline"
        } else if agent.online == Some(true) || agent.tmux_target.is_some() {
            "online"
        } else {
            "offline"
        };
        let offline_reason = if status == "offline" {
            agent
                .offline_reason
                .as_deref()
                .or(if agent.online == Some(false) {
                    Some("agent-chat-offline")
                } else {
                    Some("offline")
                })
        } else {
            None
        };
        let decision = json!({
            "name": agent.name,
            "role": agent.role.as_deref().unwrap_or("agent"),
            "capability": agent.capability,
            "runtime": agent.runtime,
            "model": agent.model,
            "server": agent.server,
            "status": status,
            "offline_reason": offline_reason,
        });
        let record = json!({
            "decision": decision,
            "runtime_profile": agent.runtime_profile,
            "home_dir": agent.home_dir,
            "workdir": agent.workdir,
            "state_dir": agent.state_dir,
        });
        records.push(canonical_record(
            agentd_core::ports::CutoverSurface::Agent,
            &agent.name,
            &agent.name,
            &record,
            &decision,
        )?);
    }
    for group in messaging.groups.values() {
        let mut members = group.members.clone();
        members.sort();
        members.dedup();
        let decision = json!({ "name": group.name, "members": members });
        let record = json!({ "decision": decision, "created_at": group.created_at });
        records.push(canonical_record(
            agentd_core::ports::CutoverSurface::Group,
            &group.name,
            &group.name,
            &record,
            &decision,
        )?);
    }
    for message in &messaging.messages {
        match message {
            ImportMessage::Direct(input) => {
                let id = input.message_id.as_deref().ok_or_else(|| {
                    StoreError::Invariant("canonical direct message id is missing".to_string())
                })?;
                let decision = json!({
                    "kind": "direct",
                    "id": id,
                    "ts": input.ts,
                    "from": input.from,
                    "to": input.to,
                    "message_type": input.message_type.as_deref().unwrap_or("human"),
                    "priority": input.priority.as_deref().unwrap_or("normal"),
                    "reply_to": input.reply_to,
                    "source": input.source.as_deref().unwrap_or("api"),
                    "source_room": input.source_room,
                    "sender_mxid": input.sender_mxid,
                    "trust_level": input.trust_level,
                    "from_id": input.from_id,
                });
                let record = json!({
                    "decision": decision,
                    "summary": input.summary,
                    "full": input.full,
                    "schema": input.schema,
                    "attachments": input.attachments,
                });
                records.push(canonical_record(
                    agentd_core::ports::CutoverSurface::Message,
                    id,
                    id,
                    &record,
                    &decision,
                )?);
            }
            ImportMessage::Group(input) => {
                let id = input.message_id.as_deref().ok_or_else(|| {
                    StoreError::Invariant("canonical group message id is missing".to_string())
                })?;
                let mut mentions = input.mentions.clone();
                mentions.sort();
                mentions.dedup();
                let decision = json!({
                    "kind": "group",
                    "id": id,
                    "ts": input.ts,
                    "from": input.from,
                    "group": input.group,
                    "message_type": input.message_type.as_deref().unwrap_or("inform"),
                    "priority": input.priority.as_deref().unwrap_or("normal"),
                    "mentions": mentions,
                    "reply_to": input.reply_to,
                    "source": input.source.as_deref().unwrap_or("api"),
                });
                let record = json!({
                    "decision": decision,
                    "summary": input.summary,
                    "full": input.full,
                    "schema": input.schema,
                    "attachments": input.attachments,
                });
                records.push(canonical_record(
                    agentd_core::ports::CutoverSurface::Message,
                    id,
                    id,
                    &record,
                    &decision,
                )?);
            }
        }
    }
    for cursor in &messaging.cursors {
        let decision = canonical_cursor_decision(cursor, &messaging.messages)?;
        records.push(canonical_record(
            agentd_core::ports::CutoverSurface::Cursor,
            &cursor.agent,
            &cursor.agent,
            &decision,
            &decision,
        )?);
    }
    for task in tasks.tasks {
        let record: Value = serde_json::from_str(&task.raw_json)?;
        let decision = json!({
            "id": task.id,
            "status": task.status,
            "priority": task.priority,
            "granularity": task.granularity,
            "assignee": task.assignee,
            "parent_id": task.parent_id,
            "labels": serde_json::from_str::<Value>(&task.labels_json)?,
            "waiting_reason": task.waiting_reason,
            "waiting_until": task.waiting_until,
        });
        records.push(canonical_record(
            agentd_core::ports::CutoverSurface::Task,
            &task.id,
            &task.id,
            &record,
            &decision,
        )?);
    }
    for graph in tasks.task_graphs {
        let record: Value = serde_json::from_str(&graph.raw_json)?;
        let decision = json!({
            "id": graph.id,
            "owner": graph.owner,
            "label": graph.label,
            "status": graph.status,
            "graph": record,
        });
        records.push(canonical_record(
            agentd_core::ports::CutoverSurface::TaskGraph,
            &graph.id,
            &graph.id,
            &record,
            &decision,
        )?);
    }
    records.sort_by(|left, right| {
        left.surface
            .as_str()
            .cmp(right.surface.as_str())
            .then_with(|| left.legacy_id.cmp(&right.legacy_id))
    });
    Ok(AgentChatCanonicalSnapshot {
        source_sha256: hex::encode(source_hasher.finalize()),
        file_count: u32::try_from(files.len())
            .map_err(|_| StoreError::Invariant("too many source files".to_string()))?,
        record_count: records.len() as u64,
        unsupported_count: (agents.skipped_agents
            + messaging.skipped_groups
            + messaging.skipped_messages
            + tasks.skipped_tasks
            + tasks.skipped_task_graphs) as u64,
        records,
    })
}

fn canonical_cursor_decision(
    cursor: &ImportCursor,
    messages: &[ImportMessage],
) -> Result<Value, StoreError> {
    let mut inbox_read_ids = Vec::new();
    if let Some(inbox_ts) = cursor.inbox_ts {
        for message in messages {
            match message {
                ImportMessage::Direct(input) if input.to.eq_ignore_ascii_case(&cursor.agent) => {
                    let id = required_message_id(input.message_id.as_deref())?;
                    let ts = required_message_ts(input.ts)?;
                    if position_is_read(ts, id, inbox_ts, cursor.inbox_id.as_deref()) {
                        inbox_read_ids.push(id.to_string());
                    }
                }
                ImportMessage::Group(input)
                    if input
                        .mentions
                        .iter()
                        .any(|mention| mention.eq_ignore_ascii_case(&cursor.agent)) =>
                {
                    let id = required_message_id(input.message_id.as_deref())?;
                    let ts = required_message_ts(input.ts)?;
                    if position_is_read(ts, id, inbox_ts, cursor.inbox_id.as_deref()) {
                        inbox_read_ids.push(id.to_string());
                    }
                }
                _ => {}
            }
        }
    }
    inbox_read_ids.sort();
    let mut group_reads = Vec::new();
    for group_cursor in &cursor.group_cursors {
        let mut message_ids = Vec::new();
        for message in messages {
            let ImportMessage::Group(input) = message else {
                continue;
            };
            if input.group != group_cursor.group {
                continue;
            }
            let id = required_message_id(input.message_id.as_deref())?;
            let ts = required_message_ts(input.ts)?;
            if position_is_read(ts, id, group_cursor.ts, group_cursor.id.as_deref()) {
                message_ids.push(id.to_string());
            }
        }
        message_ids.sort();
        group_reads.push(json!({
            "group": group_cursor.group,
            "message_ids": message_ids,
        }));
    }
    group_reads.sort_by_key(|value| {
        value
            .get("group")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    });
    Ok(json!({
        "agent": cursor.agent,
        "inbox_read_ids": inbox_read_ids,
        "group_reads": group_reads,
    }))
}

fn required_message_id(id: Option<&str>) -> Result<&str, StoreError> {
    id.ok_or_else(|| StoreError::Invariant("cursor message id is missing".to_string()))
}

fn required_message_ts(ts: Option<i64>) -> Result<i64, StoreError> {
    ts.ok_or_else(|| StoreError::Invariant("cursor message timestamp is missing".to_string()))
}

fn position_is_read(ts: i64, id: &str, cursor_ts: i64, cursor_id: Option<&str>) -> bool {
    ts < cursor_ts || (ts == cursor_ts && cursor_id.is_none_or(|cursor_id| id <= cursor_id))
}

pub fn canonical_decision_sha256(value: &Value) -> Result<String, StoreError> {
    Ok(sha256_bytes(&canonical_json_bytes(value)?))
}

pub fn agent_chat_inflight_count(agent_chat: &Path) -> Result<u64, StoreError> {
    let snapshot = TaskSourceSnapshot::read(agent_chat)?;
    let task_count = snapshot
        .tasks
        .iter()
        .filter(|task| !terminal_legacy_status(task.status.as_deref()))
        .count();
    let graph_count = snapshot
        .task_graphs
        .iter()
        .filter(|graph| !terminal_legacy_status(graph.status.as_deref()))
        .count();
    Ok((task_count + graph_count) as u64)
}

fn terminal_legacy_status(status: Option<&str>) -> bool {
    status.is_some_and(|status| {
        matches!(
            status.trim().to_ascii_lowercase().as_str(),
            "done" | "completed" | "failed" | "cancelled" | "canceled" | "skipped" | "closed"
        )
    })
}

fn canonical_record(
    surface: agentd_core::ports::CutoverSurface,
    legacy_id: &str,
    native_id: &str,
    record: &Value,
    decision: &Value,
) -> Result<CanonicalLegacyRecord, StoreError> {
    Ok(CanonicalLegacyRecord {
        surface,
        legacy_id: legacy_id.to_string(),
        native_id: native_id.to_string(),
        record_sha256: sha256_bytes(&canonical_json_bytes(record)?),
        decision_sha256: sha256_bytes(&canonical_json_bytes(decision)?),
    })
}

fn canonical_json_bytes(value: &Value) -> Result<Vec<u8>, StoreError> {
    serde_json::to_vec(&canonicalize_json(value))
        .map_err(|error| StoreError::Invariant(format!("canonical JSON failed: {error}")))
}

fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|key| (key.clone(), canonicalize_json(&object[key])))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.iter().map(canonicalize_json).collect()),
        _ => value.clone(),
    }
}

fn hash_snapshot_field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
