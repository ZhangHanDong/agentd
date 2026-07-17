//! `SQLite` implementation of the durable AD-E6 final-cutover ledger.

use agentd_core::ports::{
    BackupManifest, CursorHandoff, CutoverError, CutoverLedgerPort, CutoverPlan, CutoverRun,
    CutoverSourceManifest, CutoverState, CutoverStepReceipt, CutoverSurface, CutoverTransition,
    LegacyIdMapping, ServiceInstallation, ServiceModel, ShadowDecision,
};
use agentd_core::types::{
    BackupManifestId, CutoverId, CutoverReceiptId, CutoverSourceId, ServiceInstallationId,
};
use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone)]
pub struct SqliteCutoverLedger {
    pool: SqlitePool,
}

impl SqliteCutoverLedger {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl CutoverLedgerPort for SqliteCutoverLedger {
    async fn create_cutover(&self, plan: &CutoverPlan) -> Result<CutoverRun, CutoverError> {
        validate_plan(plan)?;
        sqlx::query(
            "INSERT OR IGNORE INTO cutover_runs \
             (id, source_root_sha256, target_database_sha256, rollback_window_expires_at, \
              state, source_id, authority_owner, record_version, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 'planned', NULL, 'agent_chat_read_only', 1, ?, ?)",
        )
        .bind(plan.id.as_str())
        .bind(&plan.source_root_sha256)
        .bind(plan.target_database_sha256.as_deref())
        .bind(plan.rollback_window_expires_at)
        .bind(plan.created_at)
        .bind(plan.created_at)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        let run = load_run(&self.pool, &plan.id)
            .await?
            .ok_or_else(|| CutoverError::Unavailable("cutover insert disappeared".to_string()))?;
        if run.plan != *plan {
            return Err(CutoverError::Conflict(
                "cutover id was replayed with different plan content".to_string(),
            ));
        }
        Ok(run)
    }

    async fn load_cutover(&self, id: &CutoverId) -> Result<Option<CutoverRun>, CutoverError> {
        load_run(&self.pool, id).await
    }

    async fn transition_cutover(
        &self,
        transition: &CutoverTransition,
    ) -> Result<CutoverRun, CutoverError> {
        validate_transition(transition)?;
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        if let Some(row) = sqlx::query(
            "SELECT expected_state, next_state, input_sha256, authority_owner, occurred_at \
             FROM cutover_transitions WHERE cutover_id = ? AND idempotency_key = ?",
        )
        .bind(transition.cutover_id.as_str())
        .bind(&transition.idempotency_key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?
        {
            let exact = row.get::<String, _>("expected_state")
                == transition.expected_state.as_str()
                && row.get::<String, _>("next_state") == transition.next_state.as_str()
                && row.get::<String, _>("input_sha256") == transition.input_sha256
                && row.get::<String, _>("authority_owner") == transition.authority_owner;
            if !exact {
                return Err(CutoverError::Conflict(
                    "cutover transition idempotency key has different content".to_string(),
                ));
            }
            tx.commit().await.map_err(db_error)?;
            return load_run(&self.pool, &transition.cutover_id)
                .await?
                .ok_or_else(|| CutoverError::NotFound(transition.cutover_id.to_string()));
        }
        let current = load_run_tx(&mut tx, &transition.cutover_id)
            .await?
            .ok_or_else(|| CutoverError::NotFound(transition.cutover_id.to_string()))?;
        if current.state != transition.expected_state {
            return Err(CutoverError::Conflict(format!(
                "cutover expected {} but is {}",
                transition.expected_state.as_str(),
                current.state.as_str()
            )));
        }
        validate_state_edge(transition.expected_state, transition.next_state)?;
        sqlx::query(
            "INSERT INTO cutover_transitions \
             (cutover_id, expected_state, next_state, idempotency_key, input_sha256, \
              authority_owner, occurred_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(transition.cutover_id.as_str())
        .bind(transition.expected_state.as_str())
        .bind(transition.next_state.as_str())
        .bind(&transition.idempotency_key)
        .bind(&transition.input_sha256)
        .bind(&transition.authority_owner)
        .bind(transition.occurred_at)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        let changed = sqlx::query(
            "UPDATE cutover_runs SET state = ?, authority_owner = ?, \
             record_version = record_version + 1, updated_at = ? WHERE id = ? AND state = ?",
        )
        .bind(transition.next_state.as_str())
        .bind(&transition.authority_owner)
        .bind(transition.occurred_at)
        .bind(transition.cutover_id.as_str())
        .bind(transition.expected_state.as_str())
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        if changed.rows_affected() != 1 {
            return Err(CutoverError::Conflict(
                "cutover state changed concurrently".to_string(),
            ));
        }
        tx.commit().await.map_err(db_error)?;
        load_run(&self.pool, &transition.cutover_id)
            .await?
            .ok_or_else(|| CutoverError::NotFound(transition.cutover_id.to_string()))
    }

    async fn record_source(
        &self,
        manifest: &CutoverSourceManifest,
    ) -> Result<CutoverSourceManifest, CutoverError> {
        if !valid_sha256(&manifest.source_sha256) {
            return Err(CutoverError::Invalid(
                "source manifest digest is invalid".to_string(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        sqlx::query(
            "INSERT OR IGNORE INTO cutover_sources \
             (id, cutover_id, source_sha256, file_count, record_count, captured_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(manifest.id.as_str())
        .bind(manifest.cutover_id.as_str())
        .bind(&manifest.source_sha256)
        .bind(i64::from(manifest.file_count))
        .bind(to_i64(manifest.record_count, "source record count")?)
        .bind(manifest.captured_at)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        let stored = load_source_tx(&mut tx, &manifest.id)
            .await?
            .ok_or_else(|| CutoverError::Unavailable("source insert disappeared".to_string()))?;
        if stored != *manifest {
            return Err(CutoverError::Conflict(
                "source manifest was replayed with different content".to_string(),
            ));
        }
        let changed = sqlx::query(
            "UPDATE cutover_runs SET source_id = COALESCE(source_id, ?), \
             record_version = CASE WHEN source_id IS NULL THEN record_version + 1 ELSE record_version END, \
             updated_at = CASE WHEN source_id IS NULL THEN ? ELSE updated_at END \
             WHERE id = ? AND (source_id IS NULL OR source_id = ?)",
        )
        .bind(manifest.id.as_str())
        .bind(manifest.captured_at)
        .bind(manifest.cutover_id.as_str())
        .bind(manifest.id.as_str())
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        if changed.rows_affected() != 1 {
            return Err(CutoverError::Conflict(
                "cutover already references a different source manifest".to_string(),
            ));
        }
        tx.commit().await.map_err(db_error)?;
        Ok(stored)
    }

    async fn record_mapping(
        &self,
        mapping: &LegacyIdMapping,
    ) -> Result<LegacyIdMapping, CutoverError> {
        validate_mapping(mapping)?;
        sqlx::query(
            "INSERT OR IGNORE INTO cutover_id_mappings \
             (cutover_id, surface, legacy_id_sha256, native_id, native_record_sha256, mapped_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(mapping.cutover_id.as_str())
        .bind(mapping.surface.as_str())
        .bind(&mapping.legacy_id_sha256)
        .bind(&mapping.native_id)
        .bind(&mapping.native_record_sha256)
        .bind(mapping.mapped_at)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        let stored = load_mapping(&self.pool, mapping).await?;
        let stored = stored.ok_or_else(|| {
            CutoverError::Unavailable("legacy id mapping disappeared".to_string())
        })?;
        if stored.cutover_id == mapping.cutover_id
            && stored.surface == mapping.surface
            && stored.legacy_id_sha256 == mapping.legacy_id_sha256
            && stored.native_id == mapping.native_id
            && stored.native_record_sha256 == mapping.native_record_sha256
        {
            Ok(stored)
        } else {
            Err(CutoverError::Conflict(
                "legacy id mapping was replayed with different content".to_string(),
            ))
        }
    }

    async fn record_shadow(
        &self,
        decision: &ShadowDecision,
    ) -> Result<ShadowDecision, CutoverError> {
        validate_shadow(decision)?;
        sqlx::query(
            "INSERT OR IGNORE INTO cutover_shadow_decisions \
             (cutover_id, surface, decision_key_sha256, legacy_decision_sha256, \
              native_decision_sha256, matched, reason_code, observed_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(decision.cutover_id.as_str())
        .bind(decision.surface.as_str())
        .bind(&decision.decision_key_sha256)
        .bind(&decision.legacy_decision_sha256)
        .bind(&decision.native_decision_sha256)
        .bind(decision.matched)
        .bind(&decision.reason_code)
        .bind(decision.observed_at)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        let stored = load_shadow(&self.pool, decision).await?;
        let stored = stored
            .ok_or_else(|| CutoverError::Unavailable("shadow decision disappeared".to_string()))?;
        if stored.cutover_id == decision.cutover_id
            && stored.surface == decision.surface
            && stored.decision_key_sha256 == decision.decision_key_sha256
            && stored.legacy_decision_sha256 == decision.legacy_decision_sha256
            && stored.native_decision_sha256 == decision.native_decision_sha256
            && stored.matched == decision.matched
            && stored.reason_code == decision.reason_code
        {
            Ok(stored)
        } else {
            Err(CutoverError::Conflict(
                "shadow decision was replayed with different content".to_string(),
            ))
        }
    }

    async fn record_step(
        &self,
        receipt: &CutoverStepReceipt,
    ) -> Result<CutoverStepReceipt, CutoverError> {
        if receipt.step.trim().is_empty()
            || receipt.idempotency_key.trim().is_empty()
            || !valid_sha256(&receipt.input_sha256)
            || !valid_sha256(&receipt.output_sha256)
        {
            return Err(CutoverError::Invalid(
                "cutover step receipt is invalid".to_string(),
            ));
        }
        sqlx::query(
            "INSERT OR IGNORE INTO cutover_step_receipts \
             (id, cutover_id, step, idempotency_key, input_sha256, output_sha256, occurred_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(receipt.id.as_str())
        .bind(receipt.cutover_id.as_str())
        .bind(&receipt.step)
        .bind(&receipt.idempotency_key)
        .bind(&receipt.input_sha256)
        .bind(&receipt.output_sha256)
        .bind(receipt.occurred_at)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        let stored = load_receipt(&self.pool, receipt).await?;
        let stored = stored.ok_or_else(|| {
            CutoverError::Unavailable("cutover step receipt disappeared".to_string())
        })?;
        if stored.cutover_id == receipt.cutover_id
            && stored.step == receipt.step
            && stored.idempotency_key == receipt.idempotency_key
            && stored.input_sha256 == receipt.input_sha256
            && stored.output_sha256 == receipt.output_sha256
        {
            Ok(stored)
        } else {
            Err(CutoverError::Conflict(
                "cutover step receipt was replayed with different content".to_string(),
            ))
        }
    }

    async fn record_cursor_handoff(
        &self,
        handoff: &CursorHandoff,
    ) -> Result<CursorHandoff, CutoverError> {
        if !valid_sha256(&handoff.project_ref_sha256)
            || !valid_sha256(&handoff.previous_cursor_sha256)
            || handoff.next_cursor.trim().is_empty()
            || !valid_authority(&handoff.authority_owner)
        {
            return Err(CutoverError::Invalid(
                "cursor handoff is invalid".to_string(),
            ));
        }
        sqlx::query(
            "INSERT OR IGNORE INTO cutover_cursor_handoffs \
             (cutover_id, project_ref_sha256, previous_cursor_sha256, next_cursor, \
              authority_owner, acknowledged, handed_off_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(handoff.cutover_id.as_str())
        .bind(&handoff.project_ref_sha256)
        .bind(&handoff.previous_cursor_sha256)
        .bind(&handoff.next_cursor)
        .bind(&handoff.authority_owner)
        .bind(handoff.acknowledged)
        .bind(handoff.handed_off_at)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        let stored = load_handoff(&self.pool, handoff).await?;
        let stored = stored
            .ok_or_else(|| CutoverError::Unavailable("cursor handoff disappeared".to_string()))?;
        if stored.cutover_id == handoff.cutover_id
            && stored.project_ref_sha256 == handoff.project_ref_sha256
            && stored.previous_cursor_sha256 == handoff.previous_cursor_sha256
            && stored.next_cursor == handoff.next_cursor
            && stored.authority_owner == handoff.authority_owner
            && stored.acknowledged == handoff.acknowledged
        {
            Ok(stored)
        } else {
            Err(CutoverError::Conflict(
                "cursor handoff was replayed with different content".to_string(),
            ))
        }
    }

    async fn record_backup(
        &self,
        manifest: &BackupManifest,
    ) -> Result<BackupManifest, CutoverError> {
        if !valid_sha256(&manifest.database_sha256)
            || manifest.schema_version == 0
            || manifest.storage_ref.trim().is_empty()
        {
            return Err(CutoverError::Invalid(
                "backup manifest is invalid".to_string(),
            ));
        }
        sqlx::query(
            "INSERT OR IGNORE INTO cutover_backup_manifests \
             (id, cutover_id, database_sha256, schema_version, size_bytes, storage_ref, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(manifest.id.as_str())
        .bind(manifest.cutover_id.as_str())
        .bind(&manifest.database_sha256)
        .bind(i64::from(manifest.schema_version))
        .bind(to_i64(manifest.size_bytes, "backup size")?)
        .bind(&manifest.storage_ref)
        .bind(manifest.created_at)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        let stored = load_backup(&self.pool, &manifest.id).await?;
        exact_or_conflict(stored, manifest, "backup manifest")
    }

    async fn record_service_installation(
        &self,
        installation: &ServiceInstallation,
    ) -> Result<ServiceInstallation, CutoverError> {
        if !valid_sha256(&installation.manifest_sha256)
            || !valid_sha256(&installation.target_ref_sha256)
        {
            return Err(CutoverError::Invalid(
                "service installation is invalid".to_string(),
            ));
        }
        sqlx::query(
            "INSERT OR IGNORE INTO cutover_service_installations \
             (id, cutover_id, model, manifest_sha256, target_ref_sha256, installed_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(installation.id.as_str())
        .bind(installation.cutover_id.as_str())
        .bind(installation.model.as_str())
        .bind(&installation.manifest_sha256)
        .bind(&installation.target_ref_sha256)
        .bind(installation.installed_at)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        let stored = load_installation(&self.pool, &installation.id).await?;
        exact_or_conflict(stored, installation, "service installation")
    }

    async fn mappings(&self, id: &CutoverId) -> Result<Vec<LegacyIdMapping>, CutoverError> {
        let rows = sqlx::query(
            "SELECT surface, legacy_id_sha256, native_id, native_record_sha256, mapped_at \
             FROM cutover_id_mappings WHERE cutover_id = ? ORDER BY surface, legacy_id_sha256",
        )
        .bind(id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        rows.into_iter()
            .map(|row| mapping_from_row(id.clone(), &row))
            .collect()
    }

    async fn shadows(&self, id: &CutoverId) -> Result<Vec<ShadowDecision>, CutoverError> {
        let rows = sqlx::query(
            "SELECT surface, decision_key_sha256, legacy_decision_sha256, \
             native_decision_sha256, matched, reason_code, observed_at \
             FROM cutover_shadow_decisions WHERE cutover_id = ? \
             ORDER BY surface, decision_key_sha256",
        )
        .bind(id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        rows.into_iter()
            .map(|row| shadow_from_row(id.clone(), &row))
            .collect()
    }

    async fn cursor_handoffs(&self, id: &CutoverId) -> Result<Vec<CursorHandoff>, CutoverError> {
        let rows = sqlx::query(
            "SELECT project_ref_sha256, previous_cursor_sha256, next_cursor, \
             authority_owner, acknowledged, handed_off_at FROM cutover_cursor_handoffs \
             WHERE cutover_id = ? ORDER BY project_ref_sha256",
        )
        .bind(id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows
            .into_iter()
            .map(|row| handoff_from_row(id.clone(), &row))
            .collect())
    }
}

async fn load_run(pool: &SqlitePool, id: &CutoverId) -> Result<Option<CutoverRun>, CutoverError> {
    let row = sqlx::query(
        "SELECT source_root_sha256, target_database_sha256, rollback_window_expires_at, \
         state, source_id, authority_owner, record_version, created_at, updated_at \
         FROM cutover_runs WHERE id = ?",
    )
    .bind(id.as_str())
    .fetch_optional(pool)
    .await
    .map_err(db_error)?;
    row.map(|row| run_from_row(id.clone(), &row)).transpose()
}

async fn load_run_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: &CutoverId,
) -> Result<Option<CutoverRun>, CutoverError> {
    let row = sqlx::query(
        "SELECT source_root_sha256, target_database_sha256, rollback_window_expires_at, \
         state, source_id, authority_owner, record_version, created_at, updated_at \
         FROM cutover_runs WHERE id = ?",
    )
    .bind(id.as_str())
    .fetch_optional(&mut **tx)
    .await
    .map_err(db_error)?;
    row.map(|row| run_from_row(id.clone(), &row)).transpose()
}

fn run_from_row(id: CutoverId, row: &sqlx::sqlite::SqliteRow) -> Result<CutoverRun, CutoverError> {
    Ok(CutoverRun {
        plan: CutoverPlan {
            id,
            source_root_sha256: row.get("source_root_sha256"),
            target_database_sha256: row.get("target_database_sha256"),
            rollback_window_expires_at: row.get("rollback_window_expires_at"),
            created_at: row.get("created_at"),
        },
        state: parse_state(&row.get::<String, _>("state"))?,
        source_id: row
            .get::<Option<String>, _>("source_id")
            .map(CutoverSourceId::from_string),
        authority_owner: row.get("authority_owner"),
        record_version: to_u64(row.get("record_version"), "record version")?,
        updated_at: row.get("updated_at"),
    })
}

async fn load_source_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: &CutoverSourceId,
) -> Result<Option<CutoverSourceManifest>, CutoverError> {
    let row = sqlx::query(
        "SELECT cutover_id, source_sha256, file_count, record_count, captured_at \
         FROM cutover_sources WHERE id = ?",
    )
    .bind(id.as_str())
    .fetch_optional(&mut **tx)
    .await
    .map_err(db_error)?;
    row.map(|row| {
        Ok(CutoverSourceManifest {
            id: id.clone(),
            cutover_id: CutoverId::from_string(row.get::<String, _>("cutover_id")),
            source_sha256: row.get("source_sha256"),
            file_count: to_u32(row.get("file_count"), "source file count")?,
            record_count: to_u64(row.get("record_count"), "source record count")?,
            captured_at: row.get("captured_at"),
        })
    })
    .transpose()
}

async fn load_mapping(
    pool: &SqlitePool,
    key: &LegacyIdMapping,
) -> Result<Option<LegacyIdMapping>, CutoverError> {
    let row = sqlx::query(
        "SELECT native_id, native_record_sha256, mapped_at FROM cutover_id_mappings \
         WHERE cutover_id = ? AND surface = ? AND legacy_id_sha256 = ?",
    )
    .bind(key.cutover_id.as_str())
    .bind(key.surface.as_str())
    .bind(&key.legacy_id_sha256)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?;
    row.map(|row| {
        mapping_from_row(key.cutover_id.clone(), &row).map(|mut mapping| {
            mapping.surface = key.surface;
            mapping.legacy_id_sha256.clone_from(&key.legacy_id_sha256);
            mapping
        })
    })
    .transpose()
}

fn mapping_from_row(
    cutover_id: CutoverId,
    row: &sqlx::sqlite::SqliteRow,
) -> Result<LegacyIdMapping, CutoverError> {
    Ok(LegacyIdMapping {
        cutover_id,
        surface: row
            .try_get::<String, _>("surface")
            .map_or(Ok(CutoverSurface::Agent), |value| parse_surface(&value))?,
        legacy_id_sha256: row.try_get("legacy_id_sha256").unwrap_or_default(),
        native_id: row.get("native_id"),
        native_record_sha256: row.get("native_record_sha256"),
        mapped_at: row.get("mapped_at"),
    })
}

async fn load_shadow(
    pool: &SqlitePool,
    key: &ShadowDecision,
) -> Result<Option<ShadowDecision>, CutoverError> {
    let row = sqlx::query(
        "SELECT legacy_decision_sha256, native_decision_sha256, matched, reason_code, observed_at \
         FROM cutover_shadow_decisions WHERE cutover_id = ? AND surface = ? \
         AND decision_key_sha256 = ?",
    )
    .bind(key.cutover_id.as_str())
    .bind(key.surface.as_str())
    .bind(&key.decision_key_sha256)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?;
    Ok(row.map(|row| ShadowDecision {
        cutover_id: key.cutover_id.clone(),
        surface: key.surface,
        decision_key_sha256: key.decision_key_sha256.clone(),
        legacy_decision_sha256: row.get("legacy_decision_sha256"),
        native_decision_sha256: row.get("native_decision_sha256"),
        matched: row.get("matched"),
        reason_code: row.get("reason_code"),
        observed_at: row.get("observed_at"),
    }))
}

fn shadow_from_row(
    cutover_id: CutoverId,
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ShadowDecision, CutoverError> {
    Ok(ShadowDecision {
        cutover_id,
        surface: parse_surface(&row.get::<String, _>("surface"))?,
        decision_key_sha256: row.get("decision_key_sha256"),
        legacy_decision_sha256: row.get("legacy_decision_sha256"),
        native_decision_sha256: row.get("native_decision_sha256"),
        matched: row.get("matched"),
        reason_code: row.get("reason_code"),
        observed_at: row.get("observed_at"),
    })
}

async fn load_receipt(
    pool: &SqlitePool,
    key: &CutoverStepReceipt,
) -> Result<Option<CutoverStepReceipt>, CutoverError> {
    let row = sqlx::query(
        "SELECT id, input_sha256, output_sha256, occurred_at FROM cutover_step_receipts \
         WHERE cutover_id = ? AND step = ? AND idempotency_key = ?",
    )
    .bind(key.cutover_id.as_str())
    .bind(&key.step)
    .bind(&key.idempotency_key)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?;
    Ok(row.map(|row| CutoverStepReceipt {
        id: CutoverReceiptId::from_string(row.get::<String, _>("id")),
        cutover_id: key.cutover_id.clone(),
        step: key.step.clone(),
        idempotency_key: key.idempotency_key.clone(),
        input_sha256: row.get("input_sha256"),
        output_sha256: row.get("output_sha256"),
        occurred_at: row.get("occurred_at"),
    }))
}

async fn load_handoff(
    pool: &SqlitePool,
    key: &CursorHandoff,
) -> Result<Option<CursorHandoff>, CutoverError> {
    let row = sqlx::query(
        "SELECT previous_cursor_sha256, next_cursor, authority_owner, acknowledged, handed_off_at \
         FROM cutover_cursor_handoffs WHERE cutover_id = ? AND project_ref_sha256 = ?",
    )
    .bind(key.cutover_id.as_str())
    .bind(&key.project_ref_sha256)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?;
    Ok(row
        .map(|row| handoff_from_row(key.cutover_id.clone(), &row))
        .map(|mut handoff| {
            handoff
                .project_ref_sha256
                .clone_from(&key.project_ref_sha256);
            handoff
        }))
}

fn handoff_from_row(cutover_id: CutoverId, row: &sqlx::sqlite::SqliteRow) -> CursorHandoff {
    CursorHandoff {
        cutover_id,
        project_ref_sha256: row.try_get("project_ref_sha256").unwrap_or_default(),
        previous_cursor_sha256: row.get("previous_cursor_sha256"),
        next_cursor: row.get("next_cursor"),
        authority_owner: row.get("authority_owner"),
        acknowledged: row.get("acknowledged"),
        handed_off_at: row.get("handed_off_at"),
    }
}

async fn load_backup(
    pool: &SqlitePool,
    id: &BackupManifestId,
) -> Result<Option<BackupManifest>, CutoverError> {
    let row = sqlx::query(
        "SELECT cutover_id, database_sha256, schema_version, size_bytes, storage_ref, created_at \
         FROM cutover_backup_manifests WHERE id = ?",
    )
    .bind(id.as_str())
    .fetch_optional(pool)
    .await
    .map_err(db_error)?;
    row.map(|row| {
        Ok(BackupManifest {
            id: id.clone(),
            cutover_id: CutoverId::from_string(row.get::<String, _>("cutover_id")),
            database_sha256: row.get("database_sha256"),
            schema_version: to_u32(row.get("schema_version"), "backup schema version")?,
            size_bytes: to_u64(row.get("size_bytes"), "backup size")?,
            storage_ref: row.get("storage_ref"),
            created_at: row.get("created_at"),
        })
    })
    .transpose()
}

async fn load_installation(
    pool: &SqlitePool,
    id: &ServiceInstallationId,
) -> Result<Option<ServiceInstallation>, CutoverError> {
    let row = sqlx::query(
        "SELECT cutover_id, model, manifest_sha256, target_ref_sha256, installed_at \
         FROM cutover_service_installations WHERE id = ?",
    )
    .bind(id.as_str())
    .fetch_optional(pool)
    .await
    .map_err(db_error)?;
    row.map(|row| {
        Ok(ServiceInstallation {
            id: id.clone(),
            cutover_id: CutoverId::from_string(row.get::<String, _>("cutover_id")),
            model: parse_service_model(&row.get::<String, _>("model"))?,
            manifest_sha256: row.get("manifest_sha256"),
            target_ref_sha256: row.get("target_ref_sha256"),
            installed_at: row.get("installed_at"),
        })
    })
    .transpose()
}

fn exact_or_conflict<T: Clone + PartialEq>(
    stored: Option<T>,
    expected: &T,
    kind: &str,
) -> Result<T, CutoverError> {
    let stored = stored.ok_or_else(|| CutoverError::Unavailable(format!("{kind} disappeared")))?;
    if stored == *expected {
        Ok(stored)
    } else {
        Err(CutoverError::Conflict(format!(
            "{kind} was replayed with different content"
        )))
    }
}

fn validate_plan(plan: &CutoverPlan) -> Result<(), CutoverError> {
    if !valid_sha256(&plan.source_root_sha256)
        || plan
            .target_database_sha256
            .as_deref()
            .is_some_and(|value| !valid_sha256(value))
        || plan.rollback_window_expires_at <= plan.created_at
        || plan.created_at < 0
    {
        return Err(CutoverError::Invalid("cutover plan is invalid".to_string()));
    }
    Ok(())
}

fn validate_transition(transition: &CutoverTransition) -> Result<(), CutoverError> {
    if transition.idempotency_key.trim().is_empty()
        || !valid_sha256(&transition.input_sha256)
        || !valid_authority(&transition.authority_owner)
        || transition.occurred_at < 0
    {
        return Err(CutoverError::Invalid(
            "cutover transition is invalid".to_string(),
        ));
    }
    Ok(())
}

fn validate_state_edge(from: CutoverState, to: CutoverState) -> Result<(), CutoverError> {
    let valid = matches!(
        (from, to),
        (CutoverState::Planned, CutoverState::Importing)
            | (
                CutoverState::Importing,
                CutoverState::Shadowing | CutoverState::RolledBack,
            )
            | (
                CutoverState::Shadowing,
                CutoverState::Draining | CutoverState::RolledBack,
            )
            | (
                CutoverState::Draining,
                CutoverState::HandoffReady | CutoverState::RolledBack,
            )
            | (
                CutoverState::HandoffReady,
                CutoverState::Active | CutoverState::RolledBack,
            )
            | (
                CutoverState::Active,
                CutoverState::Retired | CutoverState::RolledBack,
            )
    );
    if valid {
        Ok(())
    } else {
        Err(CutoverError::Conflict(format!(
            "illegal cutover transition {} -> {}",
            from.as_str(),
            to.as_str()
        )))
    }
}

fn validate_mapping(mapping: &LegacyIdMapping) -> Result<(), CutoverError> {
    if !valid_sha256(&mapping.legacy_id_sha256)
        || !valid_sha256(&mapping.native_record_sha256)
        || mapping.native_id.trim().is_empty()
    {
        return Err(CutoverError::Invalid(
            "legacy id mapping is invalid".to_string(),
        ));
    }
    Ok(())
}

fn validate_shadow(decision: &ShadowDecision) -> Result<(), CutoverError> {
    if !valid_sha256(&decision.decision_key_sha256)
        || !valid_sha256(&decision.legacy_decision_sha256)
        || !valid_sha256(&decision.native_decision_sha256)
        || decision.reason_code.trim().is_empty()
    {
        return Err(CutoverError::Invalid(
            "shadow decision is invalid".to_string(),
        ));
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_authority(value: &str) -> bool {
    matches!(value, "agent_chat_read_only" | "agentd" | "none")
}

fn parse_state(value: &str) -> Result<CutoverState, CutoverError> {
    match value {
        "planned" => Ok(CutoverState::Planned),
        "importing" => Ok(CutoverState::Importing),
        "shadowing" => Ok(CutoverState::Shadowing),
        "draining" => Ok(CutoverState::Draining),
        "handoff_ready" => Ok(CutoverState::HandoffReady),
        "active" => Ok(CutoverState::Active),
        "retired" => Ok(CutoverState::Retired),
        "rolled_back" => Ok(CutoverState::RolledBack),
        _ => Err(CutoverError::Unavailable(
            "database contains an unknown cutover state".to_string(),
        )),
    }
}

fn parse_surface(value: &str) -> Result<CutoverSurface, CutoverError> {
    match value {
        "agent" => Ok(CutoverSurface::Agent),
        "group" => Ok(CutoverSurface::Group),
        "message" => Ok(CutoverSurface::Message),
        "cursor" => Ok(CutoverSurface::Cursor),
        "task" => Ok(CutoverSurface::Task),
        "task_graph" => Ok(CutoverSurface::TaskGraph),
        "matrix_project" => Ok(CutoverSurface::MatrixProject),
        _ => Err(CutoverError::Unavailable(
            "database contains an unknown cutover surface".to_string(),
        )),
    }
}

fn parse_service_model(value: &str) -> Result<ServiceModel, CutoverError> {
    match value {
        "local" => Ok(ServiceModel::Local),
        "team" => Ok(ServiceModel::Team),
        "fleet" => Ok(ServiceModel::Fleet),
        _ => Err(CutoverError::Unavailable(
            "database contains an unknown service model".to_string(),
        )),
    }
}

fn to_i64(value: u64, field: &str) -> Result<i64, CutoverError> {
    i64::try_from(value).map_err(|_| CutoverError::Invalid(format!("{field} is too large")))
}

fn to_u64(value: i64, field: &str) -> Result<u64, CutoverError> {
    u64::try_from(value)
        .map_err(|_| CutoverError::Unavailable(format!("database {field} is invalid")))
}

fn to_u32(value: i64, field: &str) -> Result<u32, CutoverError> {
    u32::try_from(value)
        .map_err(|_| CutoverError::Unavailable(format!("database {field} is invalid")))
}

#[allow(clippy::needless_pass_by_value)]
fn db_error(error: sqlx::Error) -> CutoverError {
    CutoverError::Unavailable(format!("cutover database operation failed: {error}"))
}
