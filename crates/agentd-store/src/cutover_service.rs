//! Restartable AD-E6 import, decision-shadow, drain, handoff, and activation service.

use std::collections::BTreeMap;
use std::path::Path;

use agentd_core::ports::{
    CursorHandoff, CutoverError, CutoverLedgerPort, CutoverPlan, CutoverRun, CutoverSourceManifest,
    CutoverState, CutoverStepReceipt, CutoverSurface, CutoverTransition, LegacyIdMapping,
    ShadowDecision,
};
use agentd_core::types::{CutoverId, CutoverReceiptId, CutoverSourceId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::agent_chat_import::{
    self, AgentChatImportMode, AgentChatImportOptions, AgentChatMessageImportOptions,
    AgentChatTaskImportOptions, CanonicalLegacyRecord, agent_chat_inflight_count,
    canonical_agent_chat_snapshot, canonical_decision_sha256,
};
use crate::{SqliteCutoverLedger, SqliteStore, StoreError};

#[derive(Debug, Clone)]
pub struct CutoverService {
    store: SqliteStore,
    ledger: SqliteCutoverLedger,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CutoverImportReport {
    pub run: CutoverRun,
    pub source_sha256: String,
    pub mapped_records: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CutoverShadowMismatch {
    pub surface: CutoverSurface,
    pub decision_key_sha256: String,
    pub reason_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CutoverShadowReport {
    pub run: CutoverRun,
    pub decisions: u64,
    pub matched: u64,
    pub mismatches: Vec<CutoverShadowMismatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CutoverDrainReport {
    pub run: CutoverRun,
    pub source_inflight: u64,
    pub imported_inflight: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CutoverActivationReport {
    pub run: CutoverRun,
    pub shadow_decisions: u64,
    pub cursor_handoffs: u64,
}

impl CutoverService {
    #[must_use]
    pub fn new(store: SqliteStore) -> Self {
        let ledger = SqliteCutoverLedger::new(store.pool().clone());
        Self { store, ledger }
    }

    pub async fn plan(
        &self,
        agent_chat: &Path,
        target_database_sha256: Option<String>,
        rollback_window_expires_at: i64,
        observed_at: i64,
    ) -> Result<CutoverRun, CutoverError> {
        let snapshot = canonical_agent_chat_snapshot(agent_chat).map_err(store_error)?;
        reject_unsupported(snapshot.unsupported_count)?;
        self.ledger
            .create_cutover(&CutoverPlan {
                id: CutoverId::new(),
                source_root_sha256: snapshot.source_sha256,
                target_database_sha256,
                rollback_window_expires_at,
                created_at: observed_at,
            })
            .await
    }

    pub async fn import(
        &self,
        cutover_id: &CutoverId,
        agent_chat: &Path,
        idempotency_key: &str,
        observed_at: i64,
    ) -> Result<CutoverImportReport, CutoverError> {
        validate_idempotency_key(idempotency_key)?;
        let snapshot = canonical_agent_chat_snapshot(agent_chat).map_err(store_error)?;
        reject_unsupported(snapshot.unsupported_count)?;
        let mut run = self.require_cutover(cutover_id).await?;
        require_source_digest(&run, &snapshot.source_sha256)?;
        match run.state {
            CutoverState::Planned => {
                run = self
                    .transition(
                        &run,
                        CutoverState::Importing,
                        &format!("{idempotency_key}:begin"),
                        &snapshot.source_sha256,
                        "agent_chat_read_only",
                        observed_at,
                    )
                    .await?;
            }
            CutoverState::Importing | CutoverState::Shadowing => {}
            _ => return Err(unexpected_state(&run, "planned, importing, or shadowing")),
        }
        if run.state == CutoverState::Importing {
            agent_chat_import::import_agents_from_agent_chat(
                self.store.pool(),
                agent_chat,
                AgentChatImportOptions {
                    mode: AgentChatImportMode::Execute,
                },
            )
            .await
            .map_err(store_error)?;
            agent_chat_import::import_messages_from_agent_chat(
                self.store.pool(),
                agent_chat,
                AgentChatMessageImportOptions {
                    mode: AgentChatImportMode::Execute,
                },
            )
            .await
            .map_err(store_error)?;
            agent_chat_import::import_tasks_from_agent_chat(
                self.store.pool(),
                agent_chat,
                AgentChatTaskImportOptions {
                    mode: AgentChatImportMode::Execute,
                },
            )
            .await
            .map_err(store_error)?;

            if run.source_id.is_none() {
                self.ledger
                    .record_source(&CutoverSourceManifest {
                        id: CutoverSourceId::new(),
                        cutover_id: cutover_id.clone(),
                        source_sha256: snapshot.source_sha256.clone(),
                        file_count: snapshot.file_count,
                        record_count: snapshot.record_count,
                        captured_at: observed_at,
                    })
                    .await?;
            }
            for record in &snapshot.records {
                self.ledger
                    .record_mapping(&LegacyIdMapping {
                        cutover_id: cutover_id.clone(),
                        surface: record.surface,
                        legacy_id_sha256: sha256(record.legacy_id.as_bytes()),
                        native_id: record.native_id.clone(),
                        native_record_sha256: record.record_sha256.clone(),
                        mapped_at: observed_at,
                    })
                    .await?;
            }
            self.record_step(
                cutover_id,
                "import",
                idempotency_key,
                &snapshot.source_sha256,
                &sha256(format!("mapped:{}", snapshot.record_count).as_bytes()),
                observed_at,
            )
            .await?;
            run = self
                .transition(
                    &run,
                    CutoverState::Shadowing,
                    &format!("{idempotency_key}:complete"),
                    &snapshot.source_sha256,
                    "agent_chat_read_only",
                    observed_at,
                )
                .await?;
        }
        Ok(CutoverImportReport {
            run,
            source_sha256: snapshot.source_sha256,
            mapped_records: self.ledger.mappings(cutover_id).await?.len() as u64,
        })
    }

    pub async fn shadow(
        &self,
        cutover_id: &CutoverId,
        agent_chat: &Path,
        idempotency_key: &str,
        observed_at: i64,
    ) -> Result<CutoverShadowReport, CutoverError> {
        validate_idempotency_key(idempotency_key)?;
        let snapshot = canonical_agent_chat_snapshot(agent_chat).map_err(store_error)?;
        reject_unsupported(snapshot.unsupported_count)?;
        let mut run = self.require_cutover(cutover_id).await?;
        require_source_digest(&run, &snapshot.source_sha256)?;
        if !matches!(run.state, CutoverState::Shadowing | CutoverState::Draining) {
            return Err(unexpected_state(&run, "shadowing or draining"));
        }
        if run.state == CutoverState::Shadowing {
            for record in &snapshot.records {
                let (native_decision_sha256, reason_code) = self.native_decision(record).await?;
                let matched = native_decision_sha256 == record.decision_sha256;
                self.ledger
                    .record_shadow(&ShadowDecision {
                        cutover_id: cutover_id.clone(),
                        surface: record.surface,
                        decision_key_sha256: decision_key(record),
                        legacy_decision_sha256: record.decision_sha256.clone(),
                        native_decision_sha256,
                        matched,
                        reason_code: if matched {
                            "matched".to_string()
                        } else {
                            reason_code
                        },
                        observed_at,
                    })
                    .await?;
            }
        }
        let decisions = self.ledger.shadows(cutover_id).await?;
        let mismatches = decisions
            .iter()
            .filter(|decision| !decision.matched)
            .map(|decision| CutoverShadowMismatch {
                surface: decision.surface,
                decision_key_sha256: decision.decision_key_sha256.clone(),
                reason_code: decision.reason_code.clone(),
            })
            .collect::<Vec<_>>();
        let complete = decisions.len() == snapshot.records.len();
        if run.state == CutoverState::Shadowing && complete && mismatches.is_empty() {
            self.record_step(
                cutover_id,
                "shadow",
                idempotency_key,
                &snapshot.source_sha256,
                &sha256(format!("matched:{}", decisions.len()).as_bytes()),
                observed_at,
            )
            .await?;
            run = self
                .transition(
                    &run,
                    CutoverState::Draining,
                    &format!("{idempotency_key}:complete"),
                    &snapshot.source_sha256,
                    "agent_chat_read_only",
                    observed_at,
                )
                .await?;
        }
        Ok(CutoverShadowReport {
            run,
            decisions: decisions.len() as u64,
            matched: decisions.iter().filter(|decision| decision.matched).count() as u64,
            mismatches,
        })
    }

    pub async fn drain(
        &self,
        cutover_id: &CutoverId,
        agent_chat: &Path,
        idempotency_key: &str,
        observed_at: i64,
    ) -> Result<CutoverDrainReport, CutoverError> {
        validate_idempotency_key(idempotency_key)?;
        let snapshot = canonical_agent_chat_snapshot(agent_chat).map_err(store_error)?;
        let mut run = self.require_cutover(cutover_id).await?;
        require_source_digest(&run, &snapshot.source_sha256)?;
        if !matches!(
            run.state,
            CutoverState::Draining | CutoverState::HandoffReady
        ) {
            return Err(unexpected_state(&run, "draining or handoff_ready"));
        }
        let source_inflight = agent_chat_inflight_count(agent_chat).map_err(store_error)?;
        let imported_inflight = compatibility_inflight_count(self.store.pool()).await?;
        if run.state == CutoverState::Draining && source_inflight == 0 && imported_inflight == 0 {
            let output = sha256(b"drained:0:0");
            self.record_step(
                cutover_id,
                "drain",
                idempotency_key,
                &snapshot.source_sha256,
                &output,
                observed_at,
            )
            .await?;
            run = self
                .transition(
                    &run,
                    CutoverState::HandoffReady,
                    &format!("{idempotency_key}:complete"),
                    &output,
                    "none",
                    observed_at,
                )
                .await?;
        }
        Ok(CutoverDrainReport {
            run,
            source_inflight,
            imported_inflight,
        })
    }

    pub async fn handoff(
        &self,
        cutover_id: &CutoverId,
        handoffs: &[CursorHandoff],
        idempotency_key: &str,
        observed_at: i64,
    ) -> Result<Vec<CursorHandoff>, CutoverError> {
        validate_idempotency_key(idempotency_key)?;
        let run = self.require_cutover(cutover_id).await?;
        if run.state != CutoverState::HandoffReady {
            return Err(unexpected_state(&run, "handoff_ready"));
        }
        for handoff in handoffs {
            if handoff.cutover_id != *cutover_id
                || handoff.authority_owner != "agentd"
                || !handoff.acknowledged
            {
                return Err(CutoverError::Invalid(
                    "cursor handoff must acknowledge agentd authority".to_string(),
                ));
            }
            self.ledger.record_cursor_handoff(handoff).await?;
        }
        let persisted = self.ledger.cursor_handoffs(cutover_id).await?;
        self.record_step(
            cutover_id,
            "handoff",
            idempotency_key,
            &sha256(
                serde_json::to_vec(handoffs)
                    .map_err(invalid_json)?
                    .as_slice(),
            ),
            &sha256(format!("handoffs:{}", persisted.len()).as_bytes()),
            observed_at,
        )
        .await?;
        Ok(persisted)
    }

    pub async fn activate(
        &self,
        cutover_id: &CutoverId,
        agent_chat: &Path,
        required_project_handoffs: u32,
        idempotency_key: &str,
        observed_at: i64,
    ) -> Result<CutoverActivationReport, CutoverError> {
        validate_idempotency_key(idempotency_key)?;
        let snapshot = canonical_agent_chat_snapshot(agent_chat).map_err(store_error)?;
        let run = self.require_cutover(cutover_id).await?;
        require_source_digest(&run, &snapshot.source_sha256)?;
        if run.state != CutoverState::HandoffReady {
            return Err(unexpected_state(&run, "handoff_ready"));
        }
        let shadows = self.ledger.shadows(cutover_id).await?;
        if shadows.len() != snapshot.records.len()
            || shadows.iter().any(|decision| !decision.matched)
        {
            return Err(CutoverError::Conflict(
                "activation requires a complete drift-free shadow report".to_string(),
            ));
        }
        if agent_chat_inflight_count(agent_chat).map_err(store_error)? != 0
            || compatibility_inflight_count(self.store.pool()).await? != 0
        {
            return Err(CutoverError::Conflict(
                "activation requires every imported and source task to be terminal".to_string(),
            ));
        }
        let handoffs = self.ledger.cursor_handoffs(cutover_id).await?;
        if handoffs.len() != required_project_handoffs as usize
            || handoffs.iter().any(|handoff| !handoff.acknowledged)
        {
            return Err(CutoverError::Conflict(
                "activation requires every declared project cursor handoff".to_string(),
            ));
        }
        let output = sha256(format!("activate:{}:{}", shadows.len(), handoffs.len()).as_bytes());
        self.record_step(
            cutover_id,
            "activate",
            idempotency_key,
            &snapshot.source_sha256,
            &output,
            observed_at,
        )
        .await?;
        let run = self
            .transition(
                &run,
                CutoverState::Active,
                &format!("{idempotency_key}:complete"),
                &output,
                "agentd",
                observed_at,
            )
            .await?;
        Ok(CutoverActivationReport {
            run,
            shadow_decisions: shadows.len() as u64,
            cursor_handoffs: handoffs.len() as u64,
        })
    }

    pub async fn retire(
        &self,
        cutover_id: &CutoverId,
        idempotency_key: &str,
        observed_at: i64,
    ) -> Result<CutoverRun, CutoverError> {
        let run = self.require_cutover(cutover_id).await?;
        if run.state != CutoverState::Active {
            return Err(unexpected_state(&run, "active"));
        }
        self.transition(
            &run,
            CutoverState::Retired,
            idempotency_key,
            &sha256(b"legacy-retired"),
            "agentd",
            observed_at,
        )
        .await
    }

    pub async fn rollback(
        &self,
        cutover_id: &CutoverId,
        reason_sha256: &str,
        idempotency_key: &str,
        observed_at: i64,
    ) -> Result<CutoverRun, CutoverError> {
        validate_idempotency_key(idempotency_key)?;
        if reason_sha256.len() != 64 {
            return Err(CutoverError::Invalid(
                "rollback reason digest is invalid".to_string(),
            ));
        }
        let run = self.require_cutover(cutover_id).await?;
        if !matches!(
            run.state,
            CutoverState::Importing
                | CutoverState::Shadowing
                | CutoverState::Draining
                | CutoverState::HandoffReady
                | CutoverState::Active
        ) {
            return Err(unexpected_state(&run, "a rollback-capable state"));
        }
        self.transition(
            &run,
            CutoverState::RolledBack,
            idempotency_key,
            reason_sha256,
            "none",
            observed_at,
        )
        .await
    }

    async fn require_cutover(&self, id: &CutoverId) -> Result<CutoverRun, CutoverError> {
        self.ledger
            .load_cutover(id)
            .await?
            .ok_or_else(|| CutoverError::NotFound(id.to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    async fn transition(
        &self,
        run: &CutoverRun,
        next_state: CutoverState,
        idempotency_key: &str,
        input_sha256: &str,
        authority_owner: &str,
        occurred_at: i64,
    ) -> Result<CutoverRun, CutoverError> {
        self.ledger
            .transition_cutover(&CutoverTransition {
                cutover_id: run.plan.id.clone(),
                expected_state: run.state,
                next_state,
                idempotency_key: idempotency_key.to_string(),
                input_sha256: input_sha256.to_string(),
                authority_owner: authority_owner.to_string(),
                occurred_at,
            })
            .await
    }

    async fn record_step(
        &self,
        cutover_id: &CutoverId,
        step: &str,
        idempotency_key: &str,
        input_sha256: &str,
        output_sha256: &str,
        occurred_at: i64,
    ) -> Result<(), CutoverError> {
        self.ledger
            .record_step(&CutoverStepReceipt {
                id: CutoverReceiptId::new(),
                cutover_id: cutover_id.clone(),
                step: step.to_string(),
                idempotency_key: idempotency_key.to_string(),
                input_sha256: input_sha256.to_string(),
                output_sha256: output_sha256.to_string(),
                occurred_at,
            })
            .await
            .map(|_| ())
    }

    async fn native_decision(
        &self,
        record: &CanonicalLegacyRecord,
    ) -> Result<(String, String), CutoverError> {
        let value = match record.surface {
            CutoverSurface::Agent => {
                native_agent_decision(self.store.pool(), &record.native_id).await?
            }
            CutoverSurface::Group => {
                native_group_decision(self.store.pool(), &record.native_id).await?
            }
            CutoverSurface::Message => {
                native_message_decision(self.store.pool(), &record.native_id).await?
            }
            CutoverSurface::Cursor => {
                native_cursor_decision(self.store.pool(), &record.native_id).await?
            }
            CutoverSurface::Task => {
                native_task_decision(self.store.pool(), &record.native_id).await?
            }
            CutoverSurface::TaskGraph => {
                native_graph_decision(self.store.pool(), &record.native_id).await?
            }
            CutoverSurface::MatrixProject => None,
        };
        match value {
            Some(value) => Ok((
                canonical_decision_sha256(&value).map_err(store_error)?,
                "drift".to_string(),
            )),
            None => Ok((
                canonical_decision_sha256(&Value::Null).map_err(store_error)?,
                "missing".to_string(),
            )),
        }
    }
}

async fn native_agent_decision(
    pool: &sqlx::SqlitePool,
    id: &str,
) -> Result<Option<Value>, CutoverError> {
    let row = sqlx::query(
        "SELECT name, role, capability, runtime, model, server, status, offline_reason \
         FROM agents WHERE name = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?;
    Ok(row.map(|row| {
        json!({
            "name": row.get::<String, _>("name"),
            "role": row.get::<String, _>("role"),
            "capability": row.get::<Option<String>, _>("capability"),
            "runtime": row.get::<Option<String>, _>("runtime"),
            "model": row.get::<Option<String>, _>("model"),
            "server": row.get::<Option<String>, _>("server"),
            "status": row.get::<String, _>("status"),
            "offline_reason": row.get::<Option<String>, _>("offline_reason"),
        })
    }))
}

async fn native_group_decision(
    pool: &sqlx::SqlitePool,
    id: &str,
) -> Result<Option<Value>, CutoverError> {
    if sqlx::query("SELECT name FROM groups WHERE name = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(db_error)?
        .is_none()
    {
        return Ok(None);
    }
    let members = sqlx::query(
        "SELECT agent_name FROM group_members WHERE group_name = ? ORDER BY agent_name",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .map_err(db_error)?
    .into_iter()
    .map(|row| row.get::<String, _>("agent_name"))
    .collect::<Vec<_>>();
    Ok(Some(json!({ "name": id, "members": members })))
}

async fn native_message_decision(
    pool: &sqlx::SqlitePool,
    id: &str,
) -> Result<Option<Value>, CutoverError> {
    if let Some(row) = sqlx::query(
        "SELECT id, ts, from_agent, to_agent, message_type, priority, reply_to, source, \
         source_room, sender_mxid, trust_level, from_id FROM direct_messages WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?
    {
        return Ok(Some(json!({
            "kind": "direct",
            "id": row.get::<String, _>("id"),
            "ts": row.get::<i64, _>("ts"),
            "from": row.get::<String, _>("from_agent"),
            "to": row.get::<String, _>("to_agent"),
            "message_type": row.get::<String, _>("message_type"),
            "priority": row.get::<String, _>("priority"),
            "reply_to": row.get::<Option<String>, _>("reply_to"),
            "source": row.get::<String, _>("source"),
            "source_room": row.get::<Option<String>, _>("source_room"),
            "sender_mxid": row.get::<Option<String>, _>("sender_mxid"),
            "trust_level": row.get::<Option<String>, _>("trust_level"),
            "from_id": row.get::<Option<String>, _>("from_id"),
        })));
    }
    let row = sqlx::query(
        "SELECT id, ts, from_agent, group_name, message_type, priority, mentions_json, \
         reply_to, source FROM group_messages WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?;
    row.map(|row| {
        let mut mentions: Vec<String> =
            serde_json::from_str(&row.get::<String, _>("mentions_json")).map_err(invalid_json)?;
        mentions.sort();
        mentions.dedup();
        Ok(json!({
            "kind": "group",
            "id": row.get::<String, _>("id"),
            "ts": row.get::<i64, _>("ts"),
            "from": row.get::<String, _>("from_agent"),
            "group": row.get::<String, _>("group_name"),
            "message_type": row.get::<String, _>("message_type"),
            "priority": row.get::<String, _>("priority"),
            "mentions": mentions,
            "reply_to": row.get::<Option<String>, _>("reply_to"),
            "source": row.get::<String, _>("source"),
        }))
    })
    .transpose()
}

async fn native_cursor_decision(
    pool: &sqlx::SqlitePool,
    agent: &str,
) -> Result<Option<Value>, CutoverError> {
    let inbox_read_ids = sqlx::query(
        "SELECT id FROM (\
           SELECT id FROM direct_messages WHERE to_agent = ? AND read_at IS NOT NULL \
           UNION \
           SELECT message.id AS id FROM group_mention_reads AS reads \
           JOIN group_messages AS message ON message.id = reads.message_id WHERE reads.agent_name = ?\
         ) ORDER BY id",
    )
    .bind(agent)
    .bind(agent)
    .fetch_all(pool)
    .await
    .map_err(db_error)?
    .into_iter()
    .map(|row| row.get::<String, _>("id"))
    .collect::<Vec<_>>();
    let rows = sqlx::query(
        "SELECT reads.group_name, message.ts, message.id FROM group_message_reads AS reads \
         JOIN group_messages AS message ON message.id = reads.message_id \
         WHERE reads.agent_name = ? ORDER BY reads.group_name, message.ts DESC, message.id DESC",
    )
    .bind(agent)
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in rows {
        let group = row.get::<String, _>("group_name");
        grouped
            .entry(group)
            .or_default()
            .push(row.get::<String, _>("id"));
    }
    let group_reads = grouped
        .into_iter()
        .map(|(group, mut message_ids)| {
            message_ids.sort();
            json!({ "group": group, "message_ids": message_ids })
        })
        .collect::<Vec<_>>();
    Ok(Some(json!({
        "agent": agent,
        "inbox_read_ids": inbox_read_ids,
        "group_reads": group_reads,
    })))
}

async fn native_task_decision(
    pool: &sqlx::SqlitePool,
    id: &str,
) -> Result<Option<Value>, CutoverError> {
    let row = sqlx::query(
        "SELECT id, status, priority, granularity, assignee, parent_id, labels_json, \
         waiting_reason, waiting_until FROM agent_chat_tasks WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?;
    row.map(|row| {
        Ok(json!({
            "id": row.get::<String, _>("id"),
            "status": row.get::<Option<String>, _>("status"),
            "priority": row.get::<Option<String>, _>("priority"),
            "granularity": row.get::<Option<String>, _>("granularity"),
            "assignee": row.get::<Option<String>, _>("assignee"),
            "parent_id": row.get::<Option<String>, _>("parent_id"),
            "labels": serde_json::from_str::<Value>(&row.get::<String, _>("labels_json"))
                .map_err(invalid_json)?,
            "waiting_reason": row.get::<Option<String>, _>("waiting_reason"),
            "waiting_until": row.get::<Option<String>, _>("waiting_until"),
        }))
    })
    .transpose()
}

async fn native_graph_decision(
    pool: &sqlx::SqlitePool,
    id: &str,
) -> Result<Option<Value>, CutoverError> {
    let row = sqlx::query(
        "SELECT id, owner, label, status, raw_json FROM agent_chat_task_graphs WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?;
    row.map(|row| {
        Ok(json!({
            "id": row.get::<String, _>("id"),
            "owner": row.get::<Option<String>, _>("owner"),
            "label": row.get::<Option<String>, _>("label"),
            "status": row.get::<Option<String>, _>("status"),
            "graph": serde_json::from_str::<Value>(&row.get::<String, _>("raw_json"))
                .map_err(invalid_json)?,
        }))
    })
    .transpose()
}

async fn compatibility_inflight_count(pool: &sqlx::SqlitePool) -> Result<u64, CutoverError> {
    let task_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM agent_chat_tasks WHERE lower(COALESCE(status, '')) NOT IN \
         ('done','completed','failed','cancelled','canceled','skipped','closed')",
    )
    .fetch_one(pool)
    .await
    .map_err(db_error)?;
    let graph_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM agent_chat_task_graphs WHERE lower(COALESCE(status, '')) NOT IN \
         ('done','completed','failed','cancelled','canceled','skipped','closed')",
    )
    .fetch_one(pool)
    .await
    .map_err(db_error)?;
    u64::try_from(task_count.saturating_add(graph_count)).map_err(|_| {
        CutoverError::Unavailable("compatibility in-flight count is invalid".to_string())
    })
}

fn decision_key(record: &CanonicalLegacyRecord) -> String {
    sha256(format!("{}:{}", record.surface.as_str(), record.legacy_id).as_bytes())
}

fn require_source_digest(run: &CutoverRun, actual: &str) -> Result<(), CutoverError> {
    if run.plan.source_root_sha256 == actual {
        Ok(())
    } else {
        Err(CutoverError::Conflict(
            "agent-chat source changed after cutover planning".to_string(),
        ))
    }
}

fn reject_unsupported(count: u64) -> Result<(), CutoverError> {
    if count == 0 {
        Ok(())
    } else {
        Err(CutoverError::Invalid(format!(
            "offline source contains {count} unsupported record(s)"
        )))
    }
}

fn validate_idempotency_key(key: &str) -> Result<(), CutoverError> {
    if key.trim().is_empty() || key.len() > 512 || key.contains('\0') {
        Err(CutoverError::Invalid(
            "cutover idempotency key is invalid".to_string(),
        ))
    } else {
        Ok(())
    }
}

fn unexpected_state(run: &CutoverRun, expected: &str) -> CutoverError {
    CutoverError::Conflict(format!(
        "cutover {} is {}; expected {expected}",
        run.plan.id,
        run.state.as_str()
    ))
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn store_error(error: StoreError) -> CutoverError {
    CutoverError::Unavailable(format!("offline import operation failed: {error}"))
}

fn db_error(error: sqlx::Error) -> CutoverError {
    CutoverError::Unavailable(format!("cutover database read failed: {error}"))
}

fn invalid_json(error: serde_json::Error) -> CutoverError {
    CutoverError::Unavailable(format!("stored compatibility JSON is invalid: {error}"))
}
