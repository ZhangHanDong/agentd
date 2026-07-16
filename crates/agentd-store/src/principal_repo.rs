//! Durable enterprise principal and OIDC/Matrix identity bindings.

use agentd_core::ports::{EnterprisePrincipalPort, SecurityError};
use agentd_core::types::{
    AuthorityKey, EnterprisePrincipal, EnterprisePrincipalId, EnterpriseRequestIdentity,
    MatrixDeviceBinding, MatrixDeviceStatus, MatrixPrincipalResolveRequest, MatrixTrustPolicy,
    OidcPrincipalResolveRequest, OrganizationRef, PrincipalKind, PrincipalStatus,
    SecurityDenialReason,
};
use sqlx::{Row, SqlitePool};

use crate::error::StoreError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalUpsert {
    pub id: EnterprisePrincipalId,
    pub organization_ref: OrganizationRef,
    pub kind: PrincipalKind,
    pub display_name: String,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcSubjectBinding {
    pub issuer: String,
    pub subject: String,
    pub principal_id: EnterprisePrincipalId,
    pub bound_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixUserBinding {
    pub user_id: String,
    pub homeserver: String,
    pub principal_id: EnterprisePrincipalId,
    pub bound_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixAppserviceBinding {
    pub appservice_id: String,
    pub homeserver: String,
    pub sender_localpart_prefix: String,
    pub principal_id: EnterprisePrincipalId,
    pub bound_at: i64,
}

#[derive(Debug, Clone)]
pub struct SqliteEnterprisePrincipalRepository {
    pool: SqlitePool,
    matrix_trust_policy: MatrixTrustPolicy,
    request_ttl_seconds: i64,
}

impl SqliteEnterprisePrincipalRepository {
    pub fn new(
        pool: SqlitePool,
        matrix_trust_policy: MatrixTrustPolicy,
        request_ttl_seconds: i64,
    ) -> Result<Self, StoreError> {
        if request_ttl_seconds <= 0 {
            return Err(StoreError::Invariant(
                "enterprise request identity ttl must be positive".to_string(),
            ));
        }
        Ok(Self {
            pool,
            matrix_trust_policy,
            request_ttl_seconds,
        })
    }

    pub async fn upsert_principal(
        &self,
        request: PrincipalUpsert,
    ) -> Result<EnterprisePrincipal, StoreError> {
        validate_principal_id(&request.id)?;
        require_text(&request.display_name, "display_name")?;
        if let Some(existing) = self.get_principal_record(&request.id).await? {
            if existing.organization_ref != request.organization_ref
                || existing.kind != request.kind
            {
                return Err(StoreError::Conflict(
                    "enterprise principal immutable identity changed".to_string(),
                ));
            }
            if request.observed_at < existing.updated_at {
                return Err(StoreError::Conflict(
                    "enterprise principal update is stale".to_string(),
                ));
            }
            sqlx::query(
                "UPDATE enterprise_principals SET display_name = ?, updated_at = ? WHERE id = ?",
            )
            .bind(&request.display_name)
            .bind(request.observed_at)
            .bind(request.id.as_str())
            .execute(&self.pool)
            .await?;
            return self
                .get_principal_record(&request.id)
                .await?
                .ok_or(StoreError::NotFound);
        }

        sqlx::query(
            "INSERT INTO enterprise_principals \
             (id, organization_authority_key, organization_resource_id, \
              organization_resource_version, kind, status, display_name, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, 'active', ?, ?, ?)",
        )
        .bind(request.id.as_str())
        .bind(request.organization_ref.authority_key().as_str())
        .bind(request.organization_ref.resource_id())
        .bind(request.organization_ref.resource_version())
        .bind(principal_kind_str(request.kind))
        .bind(&request.display_name)
        .bind(request.observed_at)
        .bind(request.observed_at)
        .execute(&self.pool)
        .await?;
        self.get_principal_record(&request.id)
            .await?
            .ok_or(StoreError::NotFound)
    }

    pub async fn disable_principal(
        &self,
        id: &EnterprisePrincipalId,
        observed_at: i64,
    ) -> Result<EnterprisePrincipal, StoreError> {
        let current = self
            .get_principal_record(id)
            .await?
            .ok_or(StoreError::NotFound)?;
        if current.status == PrincipalStatus::Disabled {
            return Ok(current);
        }
        if observed_at < current.updated_at {
            return Err(StoreError::Conflict(
                "enterprise principal disable is stale".to_string(),
            ));
        }
        sqlx::query(
            "UPDATE enterprise_principals \
             SET status = 'disabled', updated_at = ?, disabled_at = ? \
             WHERE id = ? AND status = 'active'",
        )
        .bind(observed_at)
        .bind(observed_at)
        .bind(id.as_str())
        .execute(&self.pool)
        .await?;
        self.get_principal_record(id)
            .await?
            .ok_or(StoreError::NotFound)
    }

    pub async fn bind_oidc_subject(&self, binding: OidcSubjectBinding) -> Result<(), StoreError> {
        require_text(&binding.issuer, "issuer")?;
        require_text(&binding.subject, "subject")?;
        self.require_active_principal(&binding.principal_id).await?;
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT principal_id FROM oidc_principal_bindings WHERE issuer = ? AND subject = ?",
        )
        .bind(&binding.issuer)
        .bind(&binding.subject)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(existing) = existing {
            if existing == binding.principal_id.as_str() {
                return Ok(());
            }
            return Err(StoreError::Conflict(
                "OIDC issuer/subject is already bound".to_string(),
            ));
        }
        sqlx::query(
            "INSERT INTO oidc_principal_bindings (issuer, subject, principal_id, bound_at) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(&binding.issuer)
        .bind(&binding.subject)
        .bind(binding.principal_id.as_str())
        .bind(binding.bound_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn bind_matrix_user(&self, binding: MatrixUserBinding) -> Result<(), StoreError> {
        require_text(&binding.user_id, "user_id")?;
        require_text(&binding.homeserver, "homeserver")?;
        validate_matrix_user_id(&binding.user_id, &binding.homeserver)?;
        self.require_active_principal(&binding.principal_id).await?;
        let existing = sqlx::query(
            "SELECT homeserver, principal_id, status FROM matrix_principal_users WHERE user_id = ?",
        )
        .bind(&binding.user_id)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(row) = existing {
            let same = row.get::<String, _>("homeserver") == binding.homeserver
                && row.get::<String, _>("principal_id") == binding.principal_id.as_str();
            if same && row.get::<String, _>("status") == "active" {
                return Ok(());
            }
            return Err(StoreError::Conflict(
                "Matrix user is already bound or disabled".to_string(),
            ));
        }
        sqlx::query(
            "INSERT INTO matrix_principal_users \
             (user_id, homeserver, principal_id, status, bound_at, updated_at) \
             VALUES (?, ?, ?, 'active', ?, ?)",
        )
        .bind(&binding.user_id)
        .bind(&binding.homeserver)
        .bind(binding.principal_id.as_str())
        .bind(binding.bound_at)
        .bind(binding.bound_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn disable_matrix_user(
        &self,
        user_id: &str,
        observed_at: i64,
    ) -> Result<(), StoreError> {
        let updated = sqlx::query(
            "UPDATE matrix_principal_users \
             SET status = 'disabled', updated_at = ?, disabled_at = ? \
             WHERE user_id = ? AND status = 'active'",
        )
        .bind(observed_at)
        .bind(observed_at)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            let exists: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM matrix_principal_users WHERE user_id = ?")
                    .bind(user_id)
                    .fetch_one(&self.pool)
                    .await?;
            if exists == 0 {
                return Err(StoreError::NotFound);
            }
        }
        Ok(())
    }

    pub async fn bind_matrix_device(&self, binding: MatrixDeviceBinding) -> Result<(), StoreError> {
        require_text(&binding.device_id, "device_id")?;
        if binding.status != MatrixDeviceStatus::Current || binding.revoked_at.is_some() {
            return Err(StoreError::Invariant(
                "new Matrix device binding must be current".to_string(),
            ));
        }
        let user_principal: Option<String> = sqlx::query_scalar(
            "SELECT principal_id FROM matrix_principal_users \
             WHERE user_id = ? AND status = 'active'",
        )
        .bind(&binding.user_id)
        .fetch_optional(&self.pool)
        .await?;
        if user_principal.as_deref() != Some(binding.principal_id.as_str()) {
            return Err(StoreError::Conflict(
                "Matrix device principal does not match active user binding".to_string(),
            ));
        }
        let existing = sqlx::query(
            "SELECT principal_id, status FROM matrix_principal_devices \
             WHERE user_id = ? AND device_id = ?",
        )
        .bind(&binding.user_id)
        .bind(&binding.device_id)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(row) = existing {
            let same = row.get::<String, _>("principal_id") == binding.principal_id.as_str();
            if same && row.get::<String, _>("status") == "current" {
                return Ok(());
            }
            return Err(StoreError::Conflict(
                "Matrix device is already bound or revoked".to_string(),
            ));
        }
        sqlx::query(
            "INSERT INTO matrix_principal_devices \
             (user_id, device_id, principal_id, status, bound_at) \
             VALUES (?, ?, ?, 'current', ?)",
        )
        .bind(&binding.user_id)
        .bind(&binding.device_id)
        .bind(binding.principal_id.as_str())
        .bind(binding.bound_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn revoke_matrix_device(
        &self,
        user_id: &str,
        device_id: &str,
        observed_at: i64,
    ) -> Result<(), StoreError> {
        let updated = sqlx::query(
            "UPDATE matrix_principal_devices \
             SET status = 'revoked', revoked_at = ? \
             WHERE user_id = ? AND device_id = ? AND status = 'current'",
        )
        .bind(observed_at)
        .bind(user_id)
        .bind(device_id)
        .execute(&self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            let exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM matrix_principal_devices \
                 WHERE user_id = ? AND device_id = ?",
            )
            .bind(user_id)
            .bind(device_id)
            .fetch_one(&self.pool)
            .await?;
            if exists == 0 {
                return Err(StoreError::NotFound);
            }
        }
        Ok(())
    }

    pub async fn bind_matrix_appservice(
        &self,
        binding: MatrixAppserviceBinding,
    ) -> Result<(), StoreError> {
        require_text(&binding.appservice_id, "appservice_id")?;
        require_text(&binding.homeserver, "homeserver")?;
        require_text(&binding.sender_localpart_prefix, "sender_localpart_prefix")?;
        self.require_active_principal(&binding.principal_id).await?;
        let existing = sqlx::query(
            "SELECT sender_localpart_prefix, principal_id, status \
             FROM matrix_principal_appservices WHERE appservice_id = ? AND homeserver = ?",
        )
        .bind(&binding.appservice_id)
        .bind(&binding.homeserver)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(row) = existing {
            let same = row.get::<String, _>("sender_localpart_prefix")
                == binding.sender_localpart_prefix
                && row.get::<String, _>("principal_id") == binding.principal_id.as_str();
            if same && row.get::<String, _>("status") == "active" {
                return Ok(());
            }
            return Err(StoreError::Conflict(
                "Matrix appservice is already bound or disabled".to_string(),
            ));
        }
        sqlx::query(
            "INSERT INTO matrix_principal_appservices \
             (appservice_id, homeserver, sender_localpart_prefix, principal_id, status, bound_at) \
             VALUES (?, ?, ?, ?, 'active', ?)",
        )
        .bind(&binding.appservice_id)
        .bind(&binding.homeserver)
        .bind(&binding.sender_localpart_prefix)
        .bind(binding.principal_id.as_str())
        .bind(binding.bound_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn disable_matrix_appservice(
        &self,
        appservice_id: &str,
        homeserver: &str,
        observed_at: i64,
    ) -> Result<(), StoreError> {
        let updated = sqlx::query(
            "UPDATE matrix_principal_appservices \
             SET status = 'disabled', disabled_at = ? \
             WHERE appservice_id = ? AND homeserver = ? AND status = 'active'",
        )
        .bind(observed_at)
        .bind(appservice_id)
        .bind(homeserver)
        .execute(&self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            let exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM matrix_principal_appservices \
                 WHERE appservice_id = ? AND homeserver = ?",
            )
            .bind(appservice_id)
            .bind(homeserver)
            .fetch_one(&self.pool)
            .await?;
            if exists == 0 {
                return Err(StoreError::NotFound);
            }
        }
        Ok(())
    }

    async fn get_principal_record(
        &self,
        id: &EnterprisePrincipalId,
    ) -> Result<Option<EnterprisePrincipal>, StoreError> {
        let row = sqlx::query(
            "SELECT id, organization_authority_key, organization_resource_id, \
             organization_resource_version, kind, status, display_name, created_at, updated_at, \
             disabled_at FROM enterprise_principals WHERE id = ?",
        )
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(row_to_principal).transpose()
    }

    async fn require_active_principal(
        &self,
        id: &EnterprisePrincipalId,
    ) -> Result<EnterprisePrincipal, StoreError> {
        let principal = self
            .get_principal_record(id)
            .await?
            .ok_or(StoreError::NotFound)?;
        if principal.status != PrincipalStatus::Active {
            return Err(StoreError::Conflict(
                "enterprise principal is disabled".to_string(),
            ));
        }
        Ok(principal)
    }

    async fn principal_for_oidc(
        &self,
        issuer: &str,
        subject: &str,
    ) -> Result<Option<EnterprisePrincipalId>, StoreError> {
        let id: Option<String> = sqlx::query_scalar(
            "SELECT principal_id FROM oidc_principal_bindings WHERE issuer = ? AND subject = ?",
        )
        .bind(issuer)
        .bind(subject)
        .fetch_optional(&self.pool)
        .await?;
        Ok(id.map(EnterprisePrincipalId::from_string))
    }

    async fn resolve_matrix_principal_id(
        &self,
        request: &MatrixPrincipalResolveRequest,
    ) -> Result<EnterprisePrincipalId, SecurityError> {
        self.matrix_trust_policy
            .authorize_source(request)
            .map_err(SecurityError::Denied)?;
        if let Some(appservice_id) = request.appservice_id.as_ref() {
            let row = sqlx::query(
                "SELECT sender_localpart_prefix, principal_id, status \
                 FROM matrix_principal_appservices \
                 WHERE appservice_id = ? AND homeserver = ?",
            )
            .bind(appservice_id)
            .bind(&request.homeserver)
            .fetch_optional(&self.pool)
            .await
            .map_err(repository_unavailable)?
            .ok_or(SecurityError::Denied(
                SecurityDenialReason::MatrixAppserviceUntrusted,
            ))?;
            if row.get::<String, _>("status") != "active" {
                return Err(SecurityError::Denied(
                    SecurityDenialReason::MatrixAppserviceUntrusted,
                ));
            }
            let prefix: String = row.get("sender_localpart_prefix");
            let localpart = matrix_localpart(&request.user_id, &request.homeserver).ok_or(
                SecurityError::Denied(SecurityDenialReason::MatrixAppserviceUntrusted),
            )?;
            if !localpart.starts_with(&prefix) {
                return Err(SecurityError::Denied(
                    SecurityDenialReason::MatrixAppserviceUntrusted,
                ));
            }
            return Ok(EnterprisePrincipalId::from_string(
                row.get::<String, _>("principal_id"),
            ));
        }

        let row = sqlx::query(
            "SELECT principal_id, status FROM matrix_principal_users \
             WHERE user_id = ? AND homeserver = ?",
        )
        .bind(&request.user_id)
        .bind(&request.homeserver)
        .fetch_optional(&self.pool)
        .await
        .map_err(repository_unavailable)?
        .ok_or(SecurityError::Denied(
            SecurityDenialReason::PrincipalUnmapped,
        ))?;
        if row.get::<String, _>("status") != "active" {
            return Err(SecurityError::Denied(
                SecurityDenialReason::MatrixUserDisabled,
            ));
        }
        let principal_id = EnterprisePrincipalId::from_string(row.get::<String, _>("principal_id"));
        if let Some(device_id) = request.device_id.as_ref() {
            let device = sqlx::query(
                "SELECT principal_id, status FROM matrix_principal_devices \
                 WHERE user_id = ? AND device_id = ?",
            )
            .bind(&request.user_id)
            .bind(device_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(repository_unavailable)?
            .ok_or(SecurityError::Denied(
                SecurityDenialReason::PrincipalUnmapped,
            ))?;
            if device.get::<String, _>("principal_id") != principal_id.as_str() {
                return Err(SecurityError::Denied(
                    SecurityDenialReason::PrincipalUnmapped,
                ));
            }
            if device.get::<String, _>("status") != "current" {
                return Err(SecurityError::Denied(
                    SecurityDenialReason::MatrixDeviceRevoked,
                ));
            }
        }
        Ok(principal_id)
    }

    fn request_expires_at(&self, observed_at: i64) -> Result<i64, SecurityError> {
        observed_at
            .checked_add(self.request_ttl_seconds)
            .ok_or_else(|| SecurityError::Invalid("request identity expiry overflow".to_string()))
    }
}

#[async_trait::async_trait]
impl EnterprisePrincipalPort for SqliteEnterprisePrincipalRepository {
    async fn get_principal(
        &self,
        id: &EnterprisePrincipalId,
    ) -> Result<EnterprisePrincipal, SecurityError> {
        let principal = self
            .get_principal_record(id)
            .await
            .map_err(store_unavailable)?
            .ok_or(SecurityError::Denied(
                SecurityDenialReason::PrincipalUnmapped,
            ))?;
        principal.ensure_active().map_err(SecurityError::Denied)?;
        Ok(principal)
    }

    async fn resolve_oidc(
        &self,
        request: &OidcPrincipalResolveRequest,
    ) -> Result<EnterpriseRequestIdentity, SecurityError> {
        let id = self
            .principal_for_oidc(&request.issuer, &request.subject)
            .await
            .map_err(store_unavailable)?
            .ok_or(SecurityError::Denied(
                SecurityDenialReason::PrincipalUnmapped,
            ))?;
        let principal = self.get_principal(&id).await?;
        Ok(EnterpriseRequestIdentity::oidc(
            principal,
            request.clone(),
            self.request_expires_at(request.observed_at)?,
        ))
    }

    async fn resolve_matrix(
        &self,
        request: &MatrixPrincipalResolveRequest,
    ) -> Result<EnterpriseRequestIdentity, SecurityError> {
        let id = self.resolve_matrix_principal_id(request).await?;
        let principal = self.get_principal(&id).await?;
        Ok(EnterpriseRequestIdentity::matrix(
            principal,
            request.clone(),
            self.request_expires_at(request.observed_at)?,
        ))
    }
}

fn row_to_principal(row: &sqlx::sqlite::SqliteRow) -> Result<EnterprisePrincipal, StoreError> {
    let authority_key_text: String = row.get("organization_authority_key");
    let authority_key = AuthorityKey::new(authority_key_text.clone()).map_err(|error| {
        StoreError::Invariant(format!(
            "invalid enterprise principal authority key {authority_key_text}: {error}"
        ))
    })?;
    let organization_ref = OrganizationRef::new(
        authority_key,
        row.get::<String, _>("organization_resource_id"),
        row.get::<String, _>("organization_resource_version"),
    )
    .map_err(|error| StoreError::Invariant(format!("invalid organization reference: {error}")))?;
    let kind_text: String = row.get("kind");
    let kind = match kind_text.as_str() {
        "human" => PrincipalKind::Human,
        "service" => PrincipalKind::Service,
        _ => {
            return Err(StoreError::Invariant(format!(
                "invalid enterprise principal kind: {kind_text}"
            )));
        }
    };
    let status_text: String = row.get("status");
    let status = match status_text.as_str() {
        "active" => PrincipalStatus::Active,
        "disabled" => PrincipalStatus::Disabled,
        _ => {
            return Err(StoreError::Invariant(format!(
                "invalid enterprise principal status: {status_text}"
            )));
        }
    };
    Ok(EnterprisePrincipal {
        id: EnterprisePrincipalId::from_string(row.get::<String, _>("id")),
        organization_ref,
        kind,
        status,
        display_name: row.get("display_name"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        disabled_at: row.get("disabled_at"),
    })
}

fn principal_kind_str(kind: PrincipalKind) -> &'static str {
    match kind {
        PrincipalKind::Human => "human",
        PrincipalKind::Service => "service",
    }
}

fn validate_principal_id(id: &EnterprisePrincipalId) -> Result<(), StoreError> {
    let payload = id
        .as_str()
        .strip_prefix("ep_")
        .ok_or_else(|| StoreError::Invariant(format!("invalid EnterprisePrincipalId: {id:?}")))?;
    if payload.len() != 26 || payload.parse::<ulid::Ulid>().is_err() {
        return Err(StoreError::Invariant(format!(
            "invalid EnterprisePrincipalId: {id:?}"
        )));
    }
    Ok(())
}

fn require_text(value: &str, field: &str) -> Result<(), StoreError> {
    if value.trim().is_empty() || value.contains('\0') {
        return Err(StoreError::Invariant(format!("{field} is invalid")));
    }
    Ok(())
}

fn validate_matrix_user_id(user_id: &str, homeserver: &str) -> Result<(), StoreError> {
    if matrix_localpart(user_id, homeserver).is_none() {
        return Err(StoreError::Invariant(
            "Matrix user id does not match homeserver".to_string(),
        ));
    }
    Ok(())
}

fn matrix_localpart<'a>(user_id: &'a str, homeserver: &str) -> Option<&'a str> {
    let body = user_id.strip_prefix('@')?;
    let (localpart, server) = body.split_once(':')?;
    (!localpart.is_empty() && server == homeserver).then_some(localpart)
}

fn store_unavailable(_error: StoreError) -> SecurityError {
    SecurityError::Unavailable("enterprise principal repository unavailable".to_string())
}

fn repository_unavailable(_error: sqlx::Error) -> SecurityError {
    SecurityError::Unavailable("enterprise principal repository unavailable".to_string())
}
