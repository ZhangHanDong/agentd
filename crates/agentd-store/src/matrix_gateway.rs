//! Durable native Matrix gateway, command handoff, and Robrix projections.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::sync::Arc;

use agentd_core::ports::{
    MatrixCommandClass, MatrixCommandDisposition, MatrixCommandReceipt,
    MatrixExecutionSummaryStatus, MatrixGatewayCommandRequest, MatrixGatewayCutoverRequest,
    MatrixGatewayDenialReason, MatrixGatewayError, MatrixGatewayMappingKind, MatrixGatewayMode,
    MatrixGatewayOutboxRecord, MatrixGatewayPort, MatrixGatewayProjectConfig,
    MatrixGatewayRollbackManifest, MatrixGatewayStateMapping, MatrixGatewayStateMappingRequest,
    MatrixGatewaySummaryPublish, PolicyRevocationPort, RobrixApprovalView, RobrixArtifactView,
    RobrixCommandView, RobrixEvidenceView, RobrixProjectView, RobrixRunView, RobrixTaskView,
    SecurityError,
};
use agentd_core::types::{
    AuditEventId, AuthorityKey, EnterpriseAuthentication, ExecutionArtifactId, MatrixCommandId,
    MatrixGatewayOutboxId, NodeId, OrganizationRef, ProjectExecutionSnapshotRef, ProjectRef,
    ProjectRoomBindingRef, RunId, SecurityCheckpoint, SecurityEpochRequest, TaskRunId,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, SqliteConnection, SqlitePool, Transaction};

#[derive(Clone)]
pub struct SqliteMatrixGateway {
    pool: SqlitePool,
    revocation: Arc<dyn PolicyRevocationPort>,
}

impl std::fmt::Debug for SqliteMatrixGateway {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteMatrixGateway")
            .field("pool", &"[SQLITE]")
            .field("revocation", &"[CONFIGURED]")
            .finish()
    }
}

impl SqliteMatrixGateway {
    #[must_use]
    pub fn new(pool: SqlitePool, revocation: Arc<dyn PolicyRevocationPort>) -> Self {
        Self { pool, revocation }
    }

    async fn check_epoch(
        &self,
        config: &DurableBinding,
        observed_at: i64,
    ) -> Result<(), MatrixGatewayError> {
        let request = SecurityEpochRequest {
            checkpoint: SecurityCheckpoint::Dispatch,
            organization_ref: config.organization_ref.clone(),
            project_ref: config.project_ref.clone(),
            execution_snapshot_ref: config.snapshot_ref.clone(),
            pinned_epoch: config.policy_revocation_epoch,
            observed_at,
        };
        let status = self
            .revocation
            .check_security_epoch(&request)
            .await
            .map_err(|error| match error {
                SecurityError::Denied(_) | SecurityError::Invalid(_) => {
                    MatrixGatewayError::Denied(MatrixGatewayDenialReason::ProjectAuthorizationStale)
                }
                SecurityError::Unavailable(message) => MatrixGatewayError::Unavailable(message),
            })?;
        if status.observed_at > observed_at || observed_at.saturating_sub(status.observed_at) > 60 {
            return Err(MatrixGatewayError::Unavailable(
                "revocation authority returned stale or future state".to_string(),
            ));
        }
        status
            .validate_request(&request)
            .and_then(|()| status.validate_pinned_epoch(request.pinned_epoch))
            .map_err(|_| {
                MatrixGatewayError::Denied(MatrixGatewayDenialReason::ProjectAuthorizationStale)
            })
    }
}

#[async_trait::async_trait]
impl MatrixGatewayPort for SqliteMatrixGateway {
    async fn configure_project(
        &self,
        config: &MatrixGatewayProjectConfig,
    ) -> Result<RobrixProjectView, MatrixGatewayError> {
        validate_project_config(config)?;
        let room_binding = config
            .snapshot
            .room_bindings
            .iter()
            .find(|binding| binding.binding_ref == config.binding_ref)
            .ok_or(MatrixGatewayError::Denied(
                MatrixGatewayDenialReason::BindingMismatch,
            ))?;
        if room_binding.matrix_room_ref.resource_id() != config.room_id {
            return Err(MatrixGatewayError::Denied(
                MatrixGatewayDenialReason::BindingMismatch,
            ));
        }
        let allowed: BTreeSet<String> = room_binding
            .allowed_command_classes
            .iter()
            .map(|value| value.trim().to_ascii_lowercase())
            .collect();
        if allowed.is_empty()
            || allowed
                .iter()
                .any(|value| !matches!(value.as_str(), "execute" | "status" | "cancel"))
        {
            return Err(MatrixGatewayError::Invalid(
                "room binding contains unsupported command classes".to_string(),
            ));
        }
        let trusted_inviters = normalized_set(&config.trusted_inviters)?;
        let ignored_senders = normalized_set(&config.ignored_senders)?;
        let binding = config.binding_ref.as_resource_ref();
        let snapshot = &config.snapshot;
        let policy_revocation_epoch =
            i64::try_from(snapshot.policy_revocation_epoch).map_err(|_| {
                MatrixGatewayError::Invalid("policy revocation epoch exceeds bounds".to_string())
            })?;
        let mut connection = begin_immediate(&self.pool).await?;
        if let Some(row) = sqlx::query(
            "SELECT * \
             FROM matrix_gateway_project_bindings WHERE binding_authority_key = ? \
               AND binding_resource_id = ? AND binding_resource_version = ?",
        )
        .bind(binding.authority_key().as_str())
        .bind(binding.resource_id())
        .bind(binding.resource_version())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?
        {
            let exact = row.get::<String, _>("snapshot_authority_key")
                == snapshot.snapshot_ref.authority_key().as_str()
                && row.get::<String, _>("snapshot_resource_id")
                    == snapshot.snapshot_ref.resource_id()
                && row.get::<String, _>("snapshot_resource_version")
                    == snapshot.snapshot_ref.resource_version()
                && row.get::<String, _>("snapshot_content_sha256") == snapshot.content_sha256
                && row.get::<i64, _>("policy_revocation_epoch") == policy_revocation_epoch
                && row.get::<String, _>("project_authority_key")
                    == snapshot.project_ref.authority_key().as_str()
                && row.get::<String, _>("project_resource_id")
                    == snapshot.project_ref.resource_id()
                && row.get::<String, _>("project_resource_version")
                    == snapshot.project_ref.resource_version()
                && row.get::<String, _>("organization_authority_key")
                    == snapshot.organization_ref.authority_key().as_str()
                && row.get::<String, _>("organization_resource_id")
                    == snapshot.organization_ref.resource_id()
                && row.get::<String, _>("organization_resource_version")
                    == snapshot.organization_ref.resource_version()
                && row.get::<String, _>("room_id") == config.room_id;
            if !exact {
                rollback(connection).await?;
                return Err(MatrixGatewayError::Denied(
                    MatrixGatewayDenialReason::DuplicateMismatch,
                ));
            }
        }
        sqlx::query(
            "INSERT INTO matrix_gateway_project_bindings (\
                binding_authority_key, binding_resource_id, binding_resource_version, \
                project_authority_key, project_resource_id, project_resource_version, \
                organization_authority_key, organization_resource_id, organization_resource_version, \
                snapshot_authority_key, snapshot_resource_id, snapshot_resource_version, \
                snapshot_content_sha256, policy_revocation_epoch, snapshot_valid_until, \
                room_id, mode, sync_cursor, \
                allowed_command_classes_json, trusted_inviters_json, ignored_senders_json, \
                gateway_user_id, configured_at, updated_at\
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, '', ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(binding_authority_key, binding_resource_id, binding_resource_version) \
             DO UPDATE SET allowed_command_classes_json = excluded.allowed_command_classes_json, \
               trusted_inviters_json = excluded.trusted_inviters_json, \
               ignored_senders_json = excluded.ignored_senders_json, \
               gateway_user_id = excluded.gateway_user_id, updated_at = excluded.updated_at",
        )
        .bind(binding.authority_key().as_str())
        .bind(binding.resource_id())
        .bind(binding.resource_version())
        .bind(snapshot.project_ref.authority_key().as_str())
        .bind(snapshot.project_ref.resource_id())
        .bind(snapshot.project_ref.resource_version())
        .bind(snapshot.organization_ref.authority_key().as_str())
        .bind(snapshot.organization_ref.resource_id())
        .bind(snapshot.organization_ref.resource_version())
        .bind(snapshot.snapshot_ref.authority_key().as_str())
        .bind(snapshot.snapshot_ref.resource_id())
        .bind(snapshot.snapshot_ref.resource_version())
        .bind(&snapshot.content_sha256)
        .bind(policy_revocation_epoch)
        .bind(snapshot.valid_until)
        .bind(config.room_id.trim())
        .bind(config.mode.as_str())
        .bind(json(&allowed)?)
        .bind(json(&trusted_inviters)?)
        .bind(json(&ignored_senders)?)
        .bind(config.gateway_user_id.trim())
        .bind(config.configured_at)
        .bind(config.configured_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(connection).await?;
        self.project_view(&config.binding_ref, 20)
            .await?
            .ok_or_else(|| {
                MatrixGatewayError::Unavailable("configured binding missing".to_string())
            })
    }

    async fn accept_command(
        &self,
        request: &MatrixGatewayCommandRequest,
    ) -> Result<MatrixCommandReceipt, MatrixGatewayError> {
        validate_command_request(request)?;
        let transport_sha256 = sha256(&request.provenance)?;
        if let Some(existing) = existing_receipt(&self.pool, &request.provenance.event_id).await? {
            if existing.command_sha256 != request.command.command_sha256
                || existing.transport_sha256 != transport_sha256
            {
                return Err(MatrixGatewayError::Denied(
                    MatrixGatewayDenialReason::DuplicateMismatch,
                ));
            }
            let mut receipt = existing.receipt;
            receipt.disposition = MatrixCommandDisposition::Replayed;
            return Ok(receipt);
        }

        let arguments_sha256 = sha256(&request.command.arguments)?;
        let attachments_json = json(&request.command.attachments)?;
        let mut connection = begin_immediate(&self.pool).await?;

        if let Some(existing) =
            existing_receipt_connection(&mut connection, &request.provenance.event_id).await?
        {
            commit(connection).await?;
            if existing.command_sha256 != request.command.command_sha256
                || existing.transport_sha256 != transport_sha256
            {
                return Err(MatrixGatewayError::Denied(
                    MatrixGatewayDenialReason::DuplicateMismatch,
                ));
            }
            let mut receipt = existing.receipt;
            receipt.disposition = MatrixCommandDisposition::Replayed;
            return Ok(receipt);
        }

        let config = load_binding_connection(&mut connection, &request.binding_ref)
            .await?
            .ok_or_else(|| {
                MatrixGatewayError::NotFound(format!(
                    "binding {}",
                    request.binding_ref.resource_id()
                ))
            })?;
        authorize_command(&config, request)?;
        authorize_live_principal(&mut connection, &config, request).await?;
        self.check_epoch(&config, request.observed_at).await?;
        if config.sync_cursor != request.provenance.previous_sync_cursor
            && config.sync_cursor != request.provenance.sync_cursor
        {
            rollback(connection).await?;
            return Err(MatrixGatewayError::Conflict(
                "Matrix gateway cursor changed before command handoff".to_string(),
            ));
        }
        let (disposition, reason_code, create_run) =
            decide_disposition(config.mode, request.command.class);
        let command_id = MatrixCommandId::new();
        let run_id = create_run.then(RunId::new);

        if let Some(run_id) = &run_id {
            sqlx::query(
                "INSERT INTO runs (id, workflow_sha, status, started_at, last_heartbeat) \
                 VALUES (?, ?, 'running', ?, ?)",
            )
            .bind(run_id.as_str())
            .bind(&request.command.command_sha256)
            .bind(request.observed_at)
            .bind(request.observed_at)
            .execute(&mut *connection)
            .await
            .map_err(storage_error)?;
        }
        let binding = request.binding_ref.as_resource_ref();
        sqlx::query(
            "INSERT INTO matrix_gateway_commands (\
                command_id, event_id, binding_authority_key, binding_resource_id, \
                binding_resource_version, principal_id, command_class, gateway_mode, \
                command_sha256, arguments_sha256, attachments_json, disposition, reason_code, \
                run_id, accepted_at\
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(command_id.as_str())
        .bind(request.provenance.event_id.trim())
        .bind(binding.authority_key().as_str())
        .bind(binding.resource_id())
        .bind(binding.resource_version())
        .bind(request.identity.principal.id.as_str())
        .bind(request.command.class.as_str())
        .bind(config.mode.as_str())
        .bind(&request.command.command_sha256)
        .bind(arguments_sha256)
        .bind(attachments_json)
        .bind(disposition.as_str())
        .bind(reason_code)
        .bind(run_id.as_ref().map(RunId::as_str))
        .bind(request.observed_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        sqlx::query(
            "INSERT INTO matrix_gateway_inbox (\
                event_id, command_id, room_id, sender_principal_id, transport_sha256, \
                origin_server_ts, processed_at\
             ) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(request.provenance.event_id.trim())
        .bind(command_id.as_str())
        .bind(request.provenance.room_id.trim())
        .bind(request.identity.principal.id.as_str())
        .bind(&transport_sha256)
        .bind(request.provenance.origin_server_ts)
        .bind(request.observed_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;

        let outbox_id = if disposition == MatrixCommandDisposition::Accepted {
            let summary = receipt_summary(request.command.class, run_id.as_ref());
            Some(
                append_outbox(
                    &mut connection,
                    &command_id,
                    &request.provenance.room_id,
                    "command_receipt",
                    &summary,
                    &[],
                    request.observed_at,
                )
                .await?,
            )
        } else {
            None
        };
        let updated = sqlx::query(
            "UPDATE matrix_gateway_project_bindings SET previous_cursor = sync_cursor, \
             sync_cursor = ?, updated_at = ? WHERE binding_authority_key = ? \
             AND binding_resource_id = ? AND binding_resource_version = ?",
        )
        .bind(request.provenance.sync_cursor.trim())
        .bind(request.observed_at)
        .bind(binding.authority_key().as_str())
        .bind(binding.resource_id())
        .bind(binding.resource_version())
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        if updated.rows_affected() != 1 {
            rollback(connection).await?;
            return Err(MatrixGatewayError::Conflict(
                "binding changed during command handoff".to_string(),
            ));
        }
        commit(connection).await?;
        Ok(MatrixCommandReceipt {
            command_id,
            event_id: request.provenance.event_id.clone(),
            disposition,
            run_id,
            outbox_id,
            mode: config.mode,
            reason_code: reason_code.map(str::to_string),
            accepted_at: request.observed_at,
        })
    }

    async fn transition_cutover(
        &self,
        request: &MatrixGatewayCutoverRequest,
    ) -> Result<RobrixProjectView, MatrixGatewayError> {
        validate_cutover(request)?;
        if !transition_allowed(request.expected_mode, request.next_mode) {
            return Err(MatrixGatewayError::Conflict(
                "unsupported Matrix gateway cutover transition".to_string(),
            ));
        }
        let binding = request.binding_ref.as_resource_ref();
        let mut connection = begin_immediate(&self.pool).await?;
        let current = sqlx::query(
            "SELECT mode, sync_cursor FROM matrix_gateway_project_bindings \
             WHERE binding_authority_key = ? AND binding_resource_id = ? \
               AND binding_resource_version = ?",
        )
        .bind(binding.authority_key().as_str())
        .bind(binding.resource_id())
        .bind(binding.resource_version())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| MatrixGatewayError::NotFound("project binding".to_string()))?;
        if current.get::<String, _>("mode") != request.expected_mode.as_str() {
            rollback(connection).await?;
            return Err(MatrixGatewayError::Conflict(
                "Matrix gateway mode changed".to_string(),
            ));
        }
        let previous_cursor = current.get::<String, _>("sync_cursor");
        sqlx::query(
            "INSERT INTO matrix_gateway_cutover_history (\
                binding_authority_key, binding_resource_id, binding_resource_version, \
                previous_mode, next_mode, previous_cursor, next_cursor, reason_code, observed_at\
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(binding.authority_key().as_str())
        .bind(binding.resource_id())
        .bind(binding.resource_version())
        .bind(request.expected_mode.as_str())
        .bind(request.next_mode.as_str())
        .bind(&previous_cursor)
        .bind(request.cursor.trim())
        .bind(request.reason_code.trim())
        .bind(request.observed_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        sqlx::query(
            "UPDATE matrix_gateway_project_bindings SET mode = ?, previous_cursor = ?, \
             sync_cursor = ?, updated_at = ? WHERE binding_authority_key = ? \
               AND binding_resource_id = ? AND binding_resource_version = ?",
        )
        .bind(request.next_mode.as_str())
        .bind(previous_cursor)
        .bind(request.cursor.trim())
        .bind(request.observed_at)
        .bind(binding.authority_key().as_str())
        .bind(binding.resource_id())
        .bind(binding.resource_version())
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(connection).await?;
        self.project_view(&request.binding_ref, 20)
            .await?
            .ok_or_else(|| MatrixGatewayError::Unavailable("cutover binding missing".to_string()))
    }

    async fn record_state_mapping(
        &self,
        request: &MatrixGatewayStateMappingRequest,
    ) -> Result<MatrixGatewayStateMapping, MatrixGatewayError> {
        validate_sha256(&request.legacy_ref_sha256)?;
        validate_canonical_ref(&request.canonical_ref)?;
        if request.observed_at < 0 {
            return Err(MatrixGatewayError::Invalid(
                "Matrix state mapping time must be non-negative".to_string(),
            ));
        }
        let binding = request.binding_ref.as_resource_ref();
        let mut connection = begin_immediate(&self.pool).await?;
        if let Some(row) = sqlx::query(
            "SELECT canonical_ref, in_flight, observed_at FROM matrix_gateway_state_mappings \
             WHERE binding_authority_key = ? AND binding_resource_id = ? \
               AND binding_resource_version = ? AND mapping_kind = ? AND legacy_ref_sha256 = ?",
        )
        .bind(binding.authority_key().as_str())
        .bind(binding.resource_id())
        .bind(binding.resource_version())
        .bind(request.kind.as_str())
        .bind(&request.legacy_ref_sha256)
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?
        {
            let exact = row.get::<String, _>("canonical_ref") == request.canonical_ref
                && row.get::<i64, _>("in_flight") == i64::from(request.in_flight);
            if !exact {
                rollback(connection).await?;
                return Err(MatrixGatewayError::Denied(
                    MatrixGatewayDenialReason::DuplicateMismatch,
                ));
            }
            let mapping = MatrixGatewayStateMapping {
                kind: request.kind,
                legacy_ref_sha256: request.legacy_ref_sha256.clone(),
                canonical_ref: request.canonical_ref.clone(),
                in_flight: request.in_flight,
                observed_at: row.get("observed_at"),
            };
            commit(connection).await?;
            return Ok(mapping);
        }
        sqlx::query(
            "INSERT INTO matrix_gateway_state_mappings (\
                binding_authority_key, binding_resource_id, binding_resource_version, \
                mapping_kind, legacy_ref_sha256, canonical_ref, in_flight, observed_at\
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(binding.authority_key().as_str())
        .bind(binding.resource_id())
        .bind(binding.resource_version())
        .bind(request.kind.as_str())
        .bind(&request.legacy_ref_sha256)
        .bind(request.canonical_ref.trim())
        .bind(request.in_flight)
        .bind(request.observed_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(connection).await?;
        Ok(MatrixGatewayStateMapping {
            kind: request.kind,
            legacy_ref_sha256: request.legacy_ref_sha256.clone(),
            canonical_ref: request.canonical_ref.trim().to_string(),
            in_flight: request.in_flight,
            observed_at: request.observed_at,
        })
    }

    async fn rollback_manifest(
        &self,
        binding_ref: &ProjectRoomBindingRef,
    ) -> Result<MatrixGatewayRollbackManifest, MatrixGatewayError> {
        let binding = binding_ref.as_resource_ref();
        let mut connection = begin_immediate(&self.pool).await?;
        let row = sqlx::query(
            "SELECT mode, sync_cursor, previous_cursor FROM matrix_gateway_project_bindings \
             WHERE binding_authority_key = ? AND binding_resource_id = ? \
               AND binding_resource_version = ?",
        )
        .bind(binding.authority_key().as_str())
        .bind(binding.resource_id())
        .bind(binding.resource_version())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| MatrixGatewayError::NotFound("project binding".to_string()))?;
        let mapping_rows = sqlx::query(
            "SELECT mapping_kind, legacy_ref_sha256, canonical_ref, in_flight, observed_at \
             FROM matrix_gateway_state_mappings WHERE binding_authority_key = ? \
               AND binding_resource_id = ? AND binding_resource_version = ? \
             ORDER BY sequence",
        )
        .bind(binding.authority_key().as_str())
        .bind(binding.resource_id())
        .bind(binding.resource_version())
        .fetch_all(&mut *connection)
        .await
        .map_err(storage_error)?;
        let manifest = MatrixGatewayRollbackManifest {
            binding_ref: binding_ref.clone(),
            mode: parse_mode(&row.get::<String, _>("mode"))?,
            current_cursor: row.get("sync_cursor"),
            previous_cursor: row.get("previous_cursor"),
            mappings: mapping_rows
                .iter()
                .map(row_to_state_mapping)
                .collect::<Result<Vec<_>, _>>()?,
        };
        commit(connection).await?;
        Ok(manifest)
    }

    async fn publish_summary(
        &self,
        request: &MatrixGatewaySummaryPublish,
    ) -> Result<MatrixGatewayOutboxId, MatrixGatewayError> {
        validate_summary(request)?;
        let row = sqlx::query(
            "SELECT binding.room_id FROM matrix_gateway_commands AS command \
             JOIN matrix_gateway_project_bindings AS binding \
               ON binding.binding_authority_key = command.binding_authority_key \
              AND binding.binding_resource_id = command.binding_resource_id \
              AND binding.binding_resource_version = command.binding_resource_version \
             WHERE command.command_id = ?",
        )
        .bind(request.command_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| MatrixGatewayError::NotFound("command".to_string()))?;
        let mut connection = begin_immediate(&self.pool).await?;
        let summary = semantic_summary(&request.status, request.reason_code.as_deref());
        let outbox = append_outbox(
            &mut connection,
            &request.command_id,
            &row.get::<String, _>("room_id"),
            "execution_summary",
            &summary,
            &request.actionable_links,
            request.observed_at,
        )
        .await?;
        commit(connection).await?;
        Ok(outbox)
    }

    async fn outbox_after(
        &self,
        after_sequence: Option<u64>,
        limit: u32,
    ) -> Result<Vec<MatrixGatewayOutboxRecord>, MatrixGatewayError> {
        if limit == 0 || limit > 500 {
            return Err(MatrixGatewayError::Invalid(
                "Matrix gateway outbox limit must be within 1..=500".to_string(),
            ));
        }
        let after = i64::try_from(after_sequence.unwrap_or_default()).map_err(|_| {
            MatrixGatewayError::Invalid("Matrix gateway outbox cursor exceeds bounds".to_string())
        })?;
        let rows = sqlx::query(
            "SELECT sequence, outbox_id, command_id, room_id, event_kind, summary, \
                    actionable_links_json, payload_sha256, created_at, delivered_at \
             FROM matrix_gateway_outbox WHERE sequence > ? AND delivered_at IS NULL \
             ORDER BY sequence LIMIT ?",
        )
        .bind(after)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?;
        rows.iter().map(row_to_outbox).collect()
    }

    async fn mark_outbox_delivered(
        &self,
        outbox_id: &MatrixGatewayOutboxId,
        delivered_at: i64,
    ) -> Result<MatrixGatewayOutboxRecord, MatrixGatewayError> {
        if delivered_at < 0 {
            return Err(MatrixGatewayError::Invalid(
                "Matrix gateway delivery time must be non-negative".to_string(),
            ));
        }
        let mut connection = begin_immediate(&self.pool).await?;
        let row = sqlx::query(
            "SELECT sequence, outbox_id, command_id, room_id, event_kind, summary, \
                    actionable_links_json, payload_sha256, created_at, delivered_at \
             FROM matrix_gateway_outbox WHERE outbox_id = ?",
        )
        .bind(outbox_id.as_str())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| MatrixGatewayError::NotFound("Matrix gateway outbox".to_string()))?;
        let current = row_to_outbox(&row)?;
        if delivered_at < current.created_at {
            rollback(connection).await?;
            return Err(MatrixGatewayError::Invalid(
                "Matrix gateway delivery precedes outbox creation".to_string(),
            ));
        }
        if current.delivered_at.is_some() {
            commit(connection).await?;
            return Ok(current);
        }
        sqlx::query(
            "UPDATE matrix_gateway_outbox SET delivered_at = ? \
             WHERE outbox_id = ? AND delivered_at IS NULL",
        )
        .bind(delivered_at)
        .bind(outbox_id.as_str())
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(connection).await?;
        Ok(MatrixGatewayOutboxRecord {
            delivered_at: Some(delivered_at),
            ..current
        })
    }

    async fn project_view(
        &self,
        binding_ref: &ProjectRoomBindingRef,
        recent_limit: u32,
    ) -> Result<Option<RobrixProjectView>, MatrixGatewayError> {
        if recent_limit == 0 || recent_limit > 100 {
            return Err(MatrixGatewayError::Invalid(
                "recent command limit must be within 1..=100".to_string(),
            ));
        }
        let binding = binding_ref.as_resource_ref();
        let row = sqlx::query(
            "SELECT project_authority_key, project_resource_id, project_resource_version, \
                    snapshot_authority_key, snapshot_resource_id, snapshot_resource_version, \
                    room_id, mode, sync_cursor FROM matrix_gateway_project_bindings \
             WHERE binding_authority_key = ? AND binding_resource_id = ? \
               AND binding_resource_version = ?",
        )
        .bind(binding.authority_key().as_str())
        .bind(binding.resource_id())
        .bind(binding.resource_version())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let commands = sqlx::query(
            "SELECT command_id, command_class, disposition, run_id, reason_code, accepted_at \
             FROM matrix_gateway_commands WHERE binding_authority_key = ? \
               AND binding_resource_id = ? AND binding_resource_version = ? \
             ORDER BY accepted_at DESC, command_id DESC LIMIT ?",
        )
        .bind(binding.authority_key().as_str())
        .bind(binding.resource_id())
        .bind(binding.resource_version())
        .bind(i64::from(recent_limit))
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?;
        let run_rows = sqlx::query(
            "SELECT DISTINCT run.id, run.status, run.started_at, run.finished_at \
             FROM runs AS run JOIN matrix_gateway_commands AS command ON command.run_id = run.id \
             WHERE command.binding_authority_key = ? AND command.binding_resource_id = ? \
               AND command.binding_resource_version = ? \
             ORDER BY run.started_at DESC, run.id DESC LIMIT ?",
        )
        .bind(binding.authority_key().as_str())
        .bind(binding.resource_id())
        .bind(binding.resource_version())
        .bind(i64::from(recent_limit))
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?;
        let mut recent_runs = Vec::with_capacity(run_rows.len());
        for run_row in &run_rows {
            recent_runs.push(load_robrix_run(&self.pool, run_row).await?);
        }
        Ok(Some(RobrixProjectView {
            project_ref: ProjectRef::new(
                authority(&row, "project_authority_key")?,
                row.get::<String, _>("project_resource_id"),
                row.get::<String, _>("project_resource_version"),
            )
            .map_err(authority_error)?,
            binding_ref: binding_ref.clone(),
            snapshot_ref: ProjectExecutionSnapshotRef::new(
                authority(&row, "snapshot_authority_key")?,
                row.get::<String, _>("snapshot_resource_id"),
                row.get::<String, _>("snapshot_resource_version"),
            )
            .map_err(authority_error)?,
            room_id: row.get("room_id"),
            mode: parse_mode(&row.get::<String, _>("mode"))?,
            sync_cursor: row.get("sync_cursor"),
            recent_commands: commands
                .iter()
                .map(row_to_command_view)
                .collect::<Result<Vec<_>, _>>()?,
            recent_runs,
        }))
    }
}

#[derive(Debug)]
struct DurableBinding {
    binding_ref: ProjectRoomBindingRef,
    organization_ref: OrganizationRef,
    project_ref: ProjectRef,
    snapshot_ref: ProjectExecutionSnapshotRef,
    policy_revocation_epoch: u64,
    snapshot_valid_until: i64,
    room_id: String,
    mode: MatrixGatewayMode,
    sync_cursor: String,
    allowed_commands: BTreeSet<String>,
    trusted_inviters: BTreeSet<String>,
    ignored_senders: BTreeSet<String>,
    gateway_user_id: String,
}

async fn load_binding_connection(
    connection: &mut SqliteConnection,
    binding_ref: &ProjectRoomBindingRef,
) -> Result<Option<DurableBinding>, MatrixGatewayError> {
    let binding = binding_ref.as_resource_ref();
    let row = sqlx::query(
        "SELECT * FROM matrix_gateway_project_bindings WHERE binding_authority_key = ? \
         AND binding_resource_id = ? AND binding_resource_version = ?",
    )
    .bind(binding.authority_key().as_str())
    .bind(binding.resource_id())
    .bind(binding.resource_version())
    .fetch_optional(&mut *connection)
    .await
    .map_err(storage_error)?;
    row.map(|row| row_to_binding(&row)).transpose()
}

fn row_to_binding(row: &sqlx::sqlite::SqliteRow) -> Result<DurableBinding, MatrixGatewayError> {
    Ok(DurableBinding {
        binding_ref: ProjectRoomBindingRef::new(
            authority(row, "binding_authority_key")?,
            row.get::<String, _>("binding_resource_id"),
            row.get::<String, _>("binding_resource_version"),
        )
        .map_err(authority_error)?,
        organization_ref: OrganizationRef::new(
            authority(row, "organization_authority_key")?,
            row.get::<String, _>("organization_resource_id"),
            row.get::<String, _>("organization_resource_version"),
        )
        .map_err(authority_error)?,
        project_ref: ProjectRef::new(
            authority(row, "project_authority_key")?,
            row.get::<String, _>("project_resource_id"),
            row.get::<String, _>("project_resource_version"),
        )
        .map_err(authority_error)?,
        snapshot_ref: ProjectExecutionSnapshotRef::new(
            authority(row, "snapshot_authority_key")?,
            row.get::<String, _>("snapshot_resource_id"),
            row.get::<String, _>("snapshot_resource_version"),
        )
        .map_err(authority_error)?,
        policy_revocation_epoch: u64::try_from(row.get::<i64, _>("policy_revocation_epoch"))
            .map_err(|_| {
                MatrixGatewayError::Unavailable(
                    "durable Matrix policy revocation epoch is invalid".to_string(),
                )
            })?,
        snapshot_valid_until: row.get("snapshot_valid_until"),
        room_id: row.get("room_id"),
        mode: parse_mode(&row.get::<String, _>("mode"))?,
        sync_cursor: row.get("sync_cursor"),
        allowed_commands: parse_json(row, "allowed_command_classes_json")?,
        trusted_inviters: parse_json(row, "trusted_inviters_json")?,
        ignored_senders: parse_json(row, "ignored_senders_json")?,
        gateway_user_id: row.get("gateway_user_id"),
    })
}

fn authorize_command(
    config: &DurableBinding,
    request: &MatrixGatewayCommandRequest,
) -> Result<(), MatrixGatewayError> {
    let provenance = &request.provenance;
    if !provenance.transport_authenticated {
        return Err(MatrixGatewayError::Denied(
            MatrixGatewayDenialReason::TransportUnauthenticated,
        ));
    }
    if provenance.sender_user_id != provenance.authenticated_sender_user_id
        || provenance.appservice_id != provenance.authenticated_appservice_id
    {
        return Err(MatrixGatewayError::Denied(
            MatrixGatewayDenialReason::TransportIdentityMismatch,
        ));
    }
    if config.binding_ref != request.binding_ref
        || config.snapshot_ref != request.snapshot_ref
        || config.room_id != provenance.room_id
    {
        return Err(MatrixGatewayError::Denied(
            MatrixGatewayDenialReason::BindingMismatch,
        ));
    }
    if config.ignored_senders.contains(&provenance.sender_user_id) {
        return Err(MatrixGatewayError::Denied(
            MatrixGatewayDenialReason::SenderIgnored,
        ));
    }
    if provenance.sender_user_id == config.gateway_user_id
        || provenance.authenticated_appservice_id.as_deref()
            == Some(config.gateway_user_id.as_str())
    {
        return Err(MatrixGatewayError::Denied(
            MatrixGatewayDenialReason::AppserviceLoop,
        ));
    }
    if !config.trusted_inviters.is_empty()
        && provenance
            .inviter_user_id
            .as_ref()
            .is_none_or(|inviter| !config.trusted_inviters.contains(inviter))
    {
        return Err(MatrixGatewayError::Denied(
            MatrixGatewayDenialReason::InviterUntrusted,
        ));
    }
    request.identity.principal.ensure_active().map_err(|_| {
        MatrixGatewayError::Denied(MatrixGatewayDenialReason::PrincipalUnauthorized)
    })?;
    if request.identity.principal.organization_ref != config.organization_ref
        || request.observed_at >= request.identity.expires_at
        || !identity_matches(&request.identity.authentication, provenance)
    {
        return Err(MatrixGatewayError::Denied(
            MatrixGatewayDenialReason::PrincipalUnauthorized,
        ));
    }
    if request.observed_at >= config.snapshot_valid_until {
        return Err(MatrixGatewayError::Denied(
            MatrixGatewayDenialReason::SnapshotExpired,
        ));
    }
    if !config
        .allowed_commands
        .contains(request.command.class.as_str())
    {
        return Err(MatrixGatewayError::Denied(
            MatrixGatewayDenialReason::CommandNotAllowed,
        ));
    }
    Ok(())
}

async fn authorize_live_principal(
    connection: &mut SqliteConnection,
    config: &DurableBinding,
    request: &MatrixGatewayCommandRequest,
) -> Result<(), MatrixGatewayError> {
    let row = sqlx::query(
        "SELECT status, organization_authority_key, organization_resource_id, \
                organization_resource_version FROM enterprise_principals WHERE id = ?",
    )
    .bind(request.identity.principal.id.as_str())
    .fetch_optional(&mut *connection)
    .await
    .map_err(storage_error)?
    .ok_or(MatrixGatewayError::Denied(
        MatrixGatewayDenialReason::PrincipalUnauthorized,
    ))?;
    let current_organization = OrganizationRef::new(
        authority(&row, "organization_authority_key")?,
        row.get::<String, _>("organization_resource_id"),
        row.get::<String, _>("organization_resource_version"),
    )
    .map_err(authority_error)?;
    if row.get::<String, _>("status") != "active"
        || current_organization != config.organization_ref
        || current_organization != request.identity.principal.organization_ref
    {
        return Err(MatrixGatewayError::Denied(
            MatrixGatewayDenialReason::PrincipalUnauthorized,
        ));
    }
    Ok(())
}

fn identity_matches(
    authentication: &EnterpriseAuthentication,
    provenance: &agentd_core::ports::MatrixTransportProvenance,
) -> bool {
    matches!(
        authentication,
        EnterpriseAuthentication::Matrix {
            user_id,
            homeserver,
            device_id,
            appservice_id,
        } if user_id == &provenance.authenticated_sender_user_id
            && homeserver == &provenance.homeserver
            && device_id == &provenance.device_id
            && appservice_id == &provenance.authenticated_appservice_id
    )
}

fn decide_disposition(
    mode: MatrixGatewayMode,
    class: MatrixCommandClass,
) -> (MatrixCommandDisposition, Option<&'static str>, bool) {
    match mode {
        MatrixGatewayMode::Observe => (MatrixCommandDisposition::Observed, None, false),
        MatrixGatewayMode::ShadowReadOnly => (MatrixCommandDisposition::Shadowed, None, false),
        MatrixGatewayMode::Canary | MatrixGatewayMode::Active => (
            MatrixCommandDisposition::Accepted,
            None,
            class == MatrixCommandClass::Execute,
        ),
        MatrixGatewayMode::Draining if class != MatrixCommandClass::Execute => {
            (MatrixCommandDisposition::Accepted, None, false)
        }
        MatrixGatewayMode::Draining => (
            MatrixCommandDisposition::Denied,
            Some(MatrixGatewayDenialReason::SideEffectsDisabled.as_str()),
            false,
        ),
        MatrixGatewayMode::Retired | MatrixGatewayMode::RolledBack => (
            MatrixCommandDisposition::Ignored,
            Some(MatrixGatewayDenialReason::SideEffectsDisabled.as_str()),
            false,
        ),
    }
}

struct ExistingReceipt {
    receipt: MatrixCommandReceipt,
    command_sha256: String,
    transport_sha256: String,
}

async fn existing_receipt(
    pool: &SqlitePool,
    event_id: &str,
) -> Result<Option<ExistingReceipt>, MatrixGatewayError> {
    let row = sqlx::query(
        "SELECT command.command_id, command.event_id, command.disposition, command.run_id, \
                command.gateway_mode, command.reason_code, command.accepted_at, \
                command.command_sha256, inbox.transport_sha256, outbox.outbox_id \
         FROM matrix_gateway_commands AS command \
         JOIN matrix_gateway_inbox AS inbox ON inbox.command_id = command.command_id \
         LEFT JOIN matrix_gateway_outbox AS outbox \
           ON outbox.command_id = command.command_id AND outbox.event_kind = 'command_receipt' \
         WHERE command.event_id = ?",
    )
    .bind(event_id)
    .fetch_optional(pool)
    .await
    .map_err(storage_error)?;
    row.as_ref().map(row_to_existing).transpose()
}

async fn existing_receipt_connection(
    connection: &mut SqliteConnection,
    event_id: &str,
) -> Result<Option<ExistingReceipt>, MatrixGatewayError> {
    let row = sqlx::query(
        "SELECT command.command_id, command.event_id, command.disposition, command.run_id, \
                command.gateway_mode, command.reason_code, command.accepted_at, \
                command.command_sha256, inbox.transport_sha256, outbox.outbox_id \
         FROM matrix_gateway_commands AS command \
         JOIN matrix_gateway_inbox AS inbox ON inbox.command_id = command.command_id \
         LEFT JOIN matrix_gateway_outbox AS outbox \
           ON outbox.command_id = command.command_id AND outbox.event_kind = 'command_receipt' \
         WHERE command.event_id = ?",
    )
    .bind(event_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(storage_error)?;
    row.as_ref().map(row_to_existing).transpose()
}

fn row_to_existing(row: &sqlx::sqlite::SqliteRow) -> Result<ExistingReceipt, MatrixGatewayError> {
    Ok(ExistingReceipt {
        receipt: MatrixCommandReceipt {
            command_id: MatrixCommandId::from_string(row.get::<String, _>("command_id")),
            event_id: row.get("event_id"),
            disposition: parse_disposition(&row.get::<String, _>("disposition"))?,
            run_id: row
                .get::<Option<String>, _>("run_id")
                .map(RunId::from_string),
            outbox_id: row
                .get::<Option<String>, _>("outbox_id")
                .map(MatrixGatewayOutboxId::from_string),
            mode: parse_mode(&row.get::<String, _>("gateway_mode"))?,
            reason_code: row.get("reason_code"),
            accepted_at: row.get("accepted_at"),
        },
        command_sha256: row.get("command_sha256"),
        transport_sha256: row.get("transport_sha256"),
    })
}

async fn append_outbox(
    connection: &mut SqliteConnection,
    command_id: &MatrixCommandId,
    room_id: &str,
    event_kind: &str,
    summary: &str,
    links: &[String],
    observed_at: i64,
) -> Result<MatrixGatewayOutboxId, MatrixGatewayError> {
    let outbox_id = MatrixGatewayOutboxId::new();
    let links_json = json(&links)?;
    let payload_sha256 = sha256(&(command_id, room_id, event_kind, summary, links))?;
    sqlx::query(
        "INSERT INTO matrix_gateway_outbox (\
            outbox_id, command_id, room_id, event_kind, summary, actionable_links_json, \
            payload_sha256, created_at\
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(outbox_id.as_str())
    .bind(command_id.as_str())
    .bind(room_id)
    .bind(event_kind)
    .bind(summary)
    .bind(links_json)
    .bind(payload_sha256)
    .bind(observed_at)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    Ok(outbox_id)
}

fn receipt_summary(class: MatrixCommandClass, run_id: Option<&RunId>) -> String {
    run_id.map_or_else(
        || format!("{} command accepted", class.as_str()),
        |run| format!("execution accepted: agentd:run/{run}"),
    )
}

fn semantic_summary(status: &MatrixExecutionSummaryStatus, reason_code: Option<&str>) -> String {
    reason_code.map_or_else(
        || format!("execution {}", status.as_str()),
        |reason| format!("execution {}: {reason}", status.as_str()),
    )
}

fn transition_allowed(from: MatrixGatewayMode, to: MatrixGatewayMode) -> bool {
    matches!(
        (from, to),
        (
            MatrixGatewayMode::Observe,
            MatrixGatewayMode::ShadowReadOnly
        ) | (MatrixGatewayMode::ShadowReadOnly, MatrixGatewayMode::Canary)
            | (MatrixGatewayMode::Canary, MatrixGatewayMode::Active)
            | (MatrixGatewayMode::Active, MatrixGatewayMode::Draining)
            | (MatrixGatewayMode::Draining, MatrixGatewayMode::Retired)
            | (_, MatrixGatewayMode::RolledBack)
            | (MatrixGatewayMode::RolledBack, MatrixGatewayMode::Observe)
    ) && from != MatrixGatewayMode::Retired
}

fn validate_project_config(config: &MatrixGatewayProjectConfig) -> Result<(), MatrixGatewayError> {
    config
        .snapshot
        .validate()
        .map_err(|error| MatrixGatewayError::Invalid(error.to_string()))?;
    for value in [&config.room_id, &config.gateway_user_id] {
        validate_text(value, 512)?;
    }
    if config.configured_at < config.snapshot.issued_at
        || config.configured_at >= config.snapshot.valid_until
    {
        return Err(MatrixGatewayError::Denied(
            MatrixGatewayDenialReason::SnapshotExpired,
        ));
    }
    if config.mode != MatrixGatewayMode::Observe {
        return Err(MatrixGatewayError::Invalid(
            "new Matrix gateway bindings must enter through observe mode".to_string(),
        ));
    }
    Ok(())
}

fn validate_command_request(
    request: &MatrixGatewayCommandRequest,
) -> Result<(), MatrixGatewayError> {
    for value in [
        &request.provenance.event_id,
        &request.provenance.room_id,
        &request.provenance.sender_user_id,
        &request.provenance.authenticated_sender_user_id,
        &request.provenance.homeserver,
        &request.provenance.sync_cursor,
    ] {
        validate_text(value, 4096)?;
    }
    if request.provenance.previous_sync_cursor.len() > 4096 {
        return Err(MatrixGatewayError::Invalid(
            "previous Matrix sync cursor exceeds bounds".to_string(),
        ));
    }
    validate_sha256(&request.command.command_sha256)?;
    if request.command.arguments.len() > 64 || request.command.attachments.len() > 32 {
        return Err(MatrixGatewayError::Invalid(
            "Matrix command arguments or attachments exceed bounds".to_string(),
        ));
    }
    for argument in &request.command.arguments {
        validate_text(argument, 1024)?;
    }
    for attachment in &request.command.attachments {
        validate_sha256(&attachment.content_sha256)?;
        validate_text(&attachment.media_type, 256)?;
        if attachment.size_bytes == 0 || attachment.size_bytes > 100 * 1024 * 1024 {
            return Err(MatrixGatewayError::Invalid(
                "Matrix attachment size is outside bounds".to_string(),
            ));
        }
    }
    if request.observed_at < 0 || request.provenance.origin_server_ts < 0 {
        return Err(MatrixGatewayError::Invalid(
            "Matrix event time must be non-negative".to_string(),
        ));
    }
    Ok(())
}

fn validate_cutover(request: &MatrixGatewayCutoverRequest) -> Result<(), MatrixGatewayError> {
    validate_text(&request.reason_code, 256)?;
    if request.cursor.len() > 4096 || request.observed_at < 0 {
        return Err(MatrixGatewayError::Invalid(
            "invalid cutover cursor or time".to_string(),
        ));
    }
    Ok(())
}

fn validate_summary(request: &MatrixGatewaySummaryPublish) -> Result<(), MatrixGatewayError> {
    if request.actionable_links.len() > 32 || request.observed_at < 0 {
        return Err(MatrixGatewayError::Invalid(
            "summary links or time exceed bounds".to_string(),
        ));
    }
    if let Some(reason_code) = &request.reason_code {
        validate_reason_code(reason_code)?;
    }
    for link in &request.actionable_links {
        validate_text(link, 512)?;
        if !(link.starts_with("agentd:") || link.starts_with("https://")) {
            return Err(MatrixGatewayError::Invalid(
                "summary links must be actionable agentd or HTTPS references".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_reason_code(value: &str) -> Result<(), MatrixGatewayError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
    {
        return Err(MatrixGatewayError::Invalid(
            "Matrix gateway reason code is not a bounded token".to_string(),
        ));
    }
    Ok(())
}

fn normalized_set(values: &[String]) -> Result<BTreeSet<String>, MatrixGatewayError> {
    values
        .iter()
        .map(|value| {
            validate_text(value, 512)?;
            Ok(value.trim().to_string())
        })
        .collect()
}

fn row_to_command_view(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<RobrixCommandView, MatrixGatewayError> {
    Ok(RobrixCommandView {
        command_id: MatrixCommandId::from_string(row.get::<String, _>("command_id")),
        class: parse_class(&row.get::<String, _>("command_class"))?,
        disposition: parse_disposition(&row.get::<String, _>("disposition"))?,
        run_id: row
            .get::<Option<String>, _>("run_id")
            .map(RunId::from_string),
        reason_code: row.get("reason_code"),
        accepted_at: row.get("accepted_at"),
    })
}

fn row_to_outbox(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<MatrixGatewayOutboxRecord, MatrixGatewayError> {
    let sequence = u64::try_from(row.get::<i64, _>("sequence")).map_err(|_| {
        MatrixGatewayError::Unavailable("invalid durable Matrix outbox sequence".to_string())
    })?;
    Ok(MatrixGatewayOutboxRecord {
        sequence,
        outbox_id: MatrixGatewayOutboxId::from_string(row.get::<String, _>("outbox_id")),
        command_id: MatrixCommandId::from_string(row.get::<String, _>("command_id")),
        room_id: row.get("room_id"),
        event_kind: row.get("event_kind"),
        summary: row.get("summary"),
        actionable_links: parse_json(row, "actionable_links_json")?,
        payload_sha256: row.get("payload_sha256"),
        created_at: row.get("created_at"),
        delivered_at: row.get("delivered_at"),
    })
}

fn row_to_state_mapping(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<MatrixGatewayStateMapping, MatrixGatewayError> {
    Ok(MatrixGatewayStateMapping {
        kind: parse_mapping_kind(&row.get::<String, _>("mapping_kind"))?,
        legacy_ref_sha256: row.get("legacy_ref_sha256"),
        canonical_ref: row.get("canonical_ref"),
        in_flight: row.get("in_flight"),
        observed_at: row.get("observed_at"),
    })
}

async fn load_robrix_run(
    pool: &SqlitePool,
    run_row: &sqlx::sqlite::SqliteRow,
) -> Result<RobrixRunView, MatrixGatewayError> {
    let run_id = RunId::from_string(run_row.get::<String, _>("id"));
    let tasks = sqlx::query(
        "SELECT id, node_id, status, started_at, finished_at FROM task_runs \
         WHERE run_id = ? ORDER BY started_at, id LIMIT 100",
    )
    .bind(run_id.as_str())
    .fetch_all(pool)
    .await
    .map_err(storage_error)?
    .iter()
    .map(|row| RobrixTaskView {
        task_id: TaskRunId::from_string(row.get::<String, _>("id")),
        node_id: NodeId::parsed(row.get::<String, _>("node_id")),
        status: row.get("status"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
    })
    .collect();
    let artifact_rows = sqlx::query(
        "SELECT id, kind, content_sha256, size_bytes, media_type, storage_ref, created_at \
         FROM execution_artifacts WHERE execution_run_id = ? \
         ORDER BY created_at, id LIMIT 100",
    )
    .bind(run_id.as_str())
    .fetch_all(pool)
    .await
    .map_err(storage_error)?;
    let artifacts = artifact_rows
        .iter()
        .map(|row| {
            let size_bytes = u64::try_from(row.get::<i64, _>("size_bytes")).map_err(|_| {
                MatrixGatewayError::Unavailable(
                    "durable execution artifact has invalid size".to_string(),
                )
            })?;
            Ok(RobrixArtifactView {
                artifact_id: ExecutionArtifactId::from_string(row.get::<String, _>("id")),
                kind: row.get("kind"),
                content_sha256: row.get("content_sha256"),
                size_bytes,
                media_type: row.get("media_type"),
                storage_ref: row.get("storage_ref"),
                created_at: row.get("created_at"),
            })
        })
        .collect::<Result<Vec<_>, MatrixGatewayError>>()?;
    let approvals = sqlx::query(
        "SELECT id, node_id, opened_at, timeout_at, answered_at FROM human_waits \
         WHERE run_id = ? ORDER BY opened_at, id LIMIT 100",
    )
    .bind(run_id.as_str())
    .fetch_all(pool)
    .await
    .map_err(storage_error)?
    .iter()
    .map(|row| {
        let answered_at = row.get::<Option<i64>, _>("answered_at");
        RobrixApprovalView {
            approval_ref: row.get("id"),
            node_id: NodeId::parsed(row.get::<String, _>("node_id")),
            status: if answered_at.is_some() {
                "answered".to_string()
            } else {
                "pending".to_string()
            },
            opened_at: row.get("opened_at"),
            timeout_at: row.get("timeout_at"),
            answered_at,
        }
    })
    .collect();
    let evidence = sqlx::query(
        "SELECT id, event_type, payload_sha256, occurred_at, recorded_at \
         FROM execution_audit_events WHERE execution_run_id = ? \
         ORDER BY sequence LIMIT 100",
    )
    .bind(run_id.as_str())
    .fetch_all(pool)
    .await
    .map_err(storage_error)?
    .iter()
    .map(|row| RobrixEvidenceView {
        audit_event_id: AuditEventId::from_string(row.get::<String, _>("id")),
        event_type: row.get("event_type"),
        payload_sha256: row.get("payload_sha256"),
        occurred_at: row.get("occurred_at"),
        recorded_at: row.get("recorded_at"),
    })
    .collect();
    Ok(RobrixRunView {
        run_id,
        status: run_row.get("status"),
        started_at: run_row.get("started_at"),
        finished_at: run_row.get("finished_at"),
        tasks,
        artifacts,
        approvals,
        evidence,
    })
}

fn parse_mode(value: &str) -> Result<MatrixGatewayMode, MatrixGatewayError> {
    match value {
        "observe" => Ok(MatrixGatewayMode::Observe),
        "shadow_read_only" => Ok(MatrixGatewayMode::ShadowReadOnly),
        "canary" => Ok(MatrixGatewayMode::Canary),
        "active" => Ok(MatrixGatewayMode::Active),
        "draining" => Ok(MatrixGatewayMode::Draining),
        "retired" => Ok(MatrixGatewayMode::Retired),
        "rolled_back" => Ok(MatrixGatewayMode::RolledBack),
        _ => Err(MatrixGatewayError::Unavailable(
            "invalid durable Matrix gateway mode".to_string(),
        )),
    }
}

fn parse_mapping_kind(value: &str) -> Result<MatrixGatewayMappingKind, MatrixGatewayError> {
    match value {
        "project" => Ok(MatrixGatewayMappingKind::Project),
        "room" => Ok(MatrixGatewayMappingKind::Room),
        "principal" => Ok(MatrixGatewayMappingKind::Principal),
        "task" => Ok(MatrixGatewayMappingKind::Task),
        "message" => Ok(MatrixGatewayMappingKind::Message),
        "cursor" => Ok(MatrixGatewayMappingKind::Cursor),
        "run" => Ok(MatrixGatewayMappingKind::Run),
        _ => Err(MatrixGatewayError::Unavailable(
            "invalid durable Matrix state mapping kind".to_string(),
        )),
    }
}

fn parse_class(value: &str) -> Result<MatrixCommandClass, MatrixGatewayError> {
    match value {
        "execute" => Ok(MatrixCommandClass::Execute),
        "status" => Ok(MatrixCommandClass::Status),
        "cancel" => Ok(MatrixCommandClass::Cancel),
        _ => Err(MatrixGatewayError::Unavailable(
            "invalid durable Matrix command class".to_string(),
        )),
    }
}

fn parse_disposition(value: &str) -> Result<MatrixCommandDisposition, MatrixGatewayError> {
    match value {
        "accepted" => Ok(MatrixCommandDisposition::Accepted),
        "observed" => Ok(MatrixCommandDisposition::Observed),
        "shadowed" => Ok(MatrixCommandDisposition::Shadowed),
        "ignored" => Ok(MatrixCommandDisposition::Ignored),
        "denied" => Ok(MatrixCommandDisposition::Denied),
        _ => Err(MatrixGatewayError::Unavailable(
            "invalid durable Matrix command disposition".to_string(),
        )),
    }
}

async fn begin_immediate(
    pool: &SqlitePool,
) -> Result<Transaction<'static, Sqlite>, MatrixGatewayError> {
    let connection = pool.acquire().await.map_err(storage_error)?;
    Transaction::begin(connection, Some(Cow::Borrowed("BEGIN IMMEDIATE")))
        .await
        .map_err(storage_error)
}

async fn commit(connection: Transaction<'static, Sqlite>) -> Result<(), MatrixGatewayError> {
    connection.commit().await.map_err(storage_error)
}

async fn rollback(connection: Transaction<'static, Sqlite>) -> Result<(), MatrixGatewayError> {
    connection.rollback().await.map_err(storage_error)
}

fn validate_text(value: &str, max: usize) -> Result<(), MatrixGatewayError> {
    if value.trim().is_empty() || value.len() > max {
        return Err(MatrixGatewayError::Invalid(
            "Matrix gateway text is empty or exceeds bounds".to_string(),
        ));
    }
    Ok(())
}

fn validate_canonical_ref(value: &str) -> Result<(), MatrixGatewayError> {
    validate_text(value, 1024)?;
    if value.chars().any(char::is_whitespace) {
        return Err(MatrixGatewayError::Invalid(
            "Matrix state mapping canonical ref contains whitespace".to_string(),
        ));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), MatrixGatewayError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(MatrixGatewayError::Invalid(
            "Matrix gateway digest must be lowercase sha256".to_string(),
        ));
    }
    Ok(())
}

fn storage_error(error: sqlx::Error) -> MatrixGatewayError {
    MatrixGatewayError::Unavailable(format!("Matrix gateway storage: {error}"))
}

fn authority_error(error: impl std::fmt::Display) -> MatrixGatewayError {
    MatrixGatewayError::Unavailable(format!("invalid durable authority reference: {error}"))
}

fn authority(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
) -> Result<AuthorityKey, MatrixGatewayError> {
    AuthorityKey::new(row.get::<String, _>(column)).map_err(authority_error)
}

fn json(value: &impl Serialize) -> Result<String, MatrixGatewayError> {
    serde_json::to_string(value).map_err(|error| MatrixGatewayError::Invalid(error.to_string()))
}

fn parse_json<T: serde::de::DeserializeOwned>(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
) -> Result<T, MatrixGatewayError> {
    serde_json::from_str(&row.get::<String, _>(column))
        .map_err(|error| MatrixGatewayError::Unavailable(error.to_string()))
}

fn sha256(value: &impl Serialize) -> Result<String, MatrixGatewayError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| MatrixGatewayError::Invalid(error.to_string()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}
