//! Durable opaque capability metadata for lease-fenced protected actions.

use agentd_core::ports::{
    ExecutionSecurityScope, ProtectedAction, ProtectedResource, TaskLeasePort,
};
use agentd_core::types::{ProjectExecutionSnapshot, TaskLeaseClaim};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use ulid::Ulid;

use crate::error::StoreError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedCapability {
    pub id: String,
    /// Returned only to the caller; only its digest is stored.
    pub token: String,
    pub expires_at: i64,
}

/// Derive the worker execution scope from an immutable authority snapshot and
/// the current fenced lease. This is intentionally pure: persistence and lease
/// validation remain the responsibility of the caller.
pub fn scope_for_snapshot(
    snapshot: &ProjectExecutionSnapshot,
    claim: TaskLeaseClaim,
) -> ExecutionSecurityScope {
    ExecutionSecurityScope {
        authority_key: snapshot.authority_key.clone(),
        organization_ref: snapshot.organization_ref.clone(),
        project_ref: snapshot.project_ref.clone(),
        snapshot_ref: snapshot.snapshot_ref.clone(),
        rbac_policy_version_ref: snapshot.rbac_policy_version_ref.clone(),
        worker_incarnation_id: claim.worker_incarnation_id.clone(),
        lease_claim: claim,
        sandbox_profile: "native-default".into(),
        egress_profile: "project-default".into(),
        policy_revocation_epoch: 0,
        valid_from: snapshot.issued_at,
        valid_until: snapshot.valid_until,
    }
}

pub async fn issue(
    pool: &SqlitePool,
    action: ProtectedAction,
    resource: &ProtectedResource,
    scope: &ExecutionSecurityScope,
    issued_at: i64,
    expires_at: i64,
) -> Result<IssuedCapability, StoreError> {
    scope
        .validate()
        .map_err(|error| StoreError::Invariant(error.to_string()))?;
    if expires_at <= issued_at || expires_at > scope.valid_until {
        return Err(StoreError::Invariant(
            "capability validity exceeds its security scope".to_string(),
        ));
    }
    let id = format!("cap_{}", Ulid::new());
    let token = format!("{}{}", Ulid::new(), Ulid::new());
    let token_digest = digest(&token);
    let resource_json = serde_json::to_string(resource)?;
    let scope_json = serde_json::to_string(scope)?;
    sqlx::query(
        "INSERT INTO execution_capabilities \
         (id, token_digest, worker_incarnation_id, lease_id, fencing_token, action, resource_json, \
          scope_json, issued_at, expires_at, revocation_epoch) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(token_digest)
    .bind(scope.worker_incarnation_id.to_string())
    .bind(scope.lease_claim.lease_id.to_string())
    .bind(i64::try_from(scope.lease_claim.fencing_token.value()).map_err(|_| {
        StoreError::Invariant("fencing token exceeds SQLite range".to_string())
    })?)
    .bind(action.as_str())
    .bind(resource_json)
    .bind(scope_json)
    .bind(issued_at)
    .bind(expires_at)
    .bind(i64::try_from(scope.policy_revocation_epoch).map_err(|_| {
        StoreError::Invariant("revocation epoch exceeds SQLite range".to_string())
    })?)
    .execute(pool)
    .await?;
    Ok(IssuedCapability {
        id,
        token,
        expires_at,
    })
}

pub async fn revoke(pool: &SqlitePool, id: &str, revoked_at: i64) -> Result<(), StoreError> {
    let result = sqlx::query(
        "UPDATE execution_capabilities SET revoked_at = COALESCE(revoked_at, ?) WHERE id = ?",
    )
    .bind(revoked_at)
    .bind(id)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(StoreError::NotFound);
    }
    Ok(())
}

pub async fn validate(
    pool: &SqlitePool,
    lease_port: &impl TaskLeasePort,
    id: &str,
    token: &str,
    action: ProtectedAction,
    resource: &ProtectedResource,
    observed_at: i64,
) -> Result<ExecutionSecurityScope, StoreError> {
    let row = sqlx::query_as::<_, (String, String, String, String, i64, Option<i64>)>(
        "SELECT token_digest, action, resource_json, scope_json, expires_at, revoked_at \
         FROM execution_capabilities WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or(StoreError::NotFound)?;
    if !constant_time_equal(row.0.as_bytes(), digest(token).as_bytes()) {
        return Err(StoreError::Conflict(
            "capability token rejected".to_string(),
        ));
    }
    if row.5.is_some() {
        return Err(StoreError::Conflict("capability revoked".to_string()));
    }
    if observed_at >= row.4 {
        return Err(StoreError::Conflict("capability expired".to_string()));
    }
    if row.1 != action.as_str() || row.2 != serde_json::to_string(resource)? {
        return Err(StoreError::Conflict(
            "capability scope does not match requested action or resource".to_string(),
        ));
    }
    let scope: ExecutionSecurityScope = serde_json::from_str(&row.3)?;
    lease_port
        .validate_claim(&scope.lease_claim, observed_at)
        .await
        .map_err(|error| StoreError::Conflict(format!("capability lease rejected: {error}")))?;
    Ok(scope)
}

fn digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::constant_time_equal;

    #[test]
    fn constant_time_equal_requires_same_bytes_and_length() {
        assert!(constant_time_equal(b"abc", b"abc"));
        assert!(!constant_time_equal(b"abc", b"abd"));
        assert!(!constant_time_equal(b"abc", b"ab"));
    }
}
