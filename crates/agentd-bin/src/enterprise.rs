//! Enterprise Specify and control-plane leadership lifecycle.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agentd_core::ports::{
    ControlPlaneHeartbeatRequest, ControlPlaneLeadershipLease, ControlPlaneLeadershipRenewal,
    ControlPlaneLeadershipRequest, ControlPlaneMember, ControlPlaneMemberStatus,
    EnterpriseScaleError, EnterpriseScalePort, PolicyRevocationPort, ProjectAuthorityAvailability,
    ProjectAuthorityPort, SecurityError,
};
use agentd_core::types::{
    AuthorityKey, ControlPlaneInstanceId, SecurityDenialReason, SecurityEpochRequest,
    SecurityEpochStatus,
};
use agentd_project_authority::{HttpSpecifyAuthorityTransport, SpecifyProjectAuthority};
use agentd_store::{SqliteEnterpriseScaleControlPlane, SqliteStore};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::{RwLock, watch};
use tokio::task::JoinHandle;
use zeroize::Zeroizing;

use crate::cli::DaemonConfig;
use crate::security::SecurityRuntimeMode;

const MAX_AUTHORIZATION_BYTES: u64 = 16 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub struct EnterpriseControlPlaneConfig {
    pub instance_id: ControlPlaneInstanceId,
    pub region: String,
    pub zone: String,
    pub endpoint_sha256: String,
    pub specify_url: String,
    pub specify_authority_key: AuthorityKey,
    pub specify_authorization: Zeroizing<String>,
    pub allow_loopback_specify_http: bool,
    pub heartbeat_interval: Duration,
    pub leadership_lease: Duration,
}

impl std::fmt::Debug for EnterpriseControlPlaneConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EnterpriseControlPlaneConfig")
            .field("instance_id", &self.instance_id)
            .field("region", &self.region)
            .field("zone", &self.zone)
            .field("endpoint_sha256", &self.endpoint_sha256)
            .field("specify_url", &self.specify_url)
            .field("specify_authority_key", &self.specify_authority_key)
            .field("specify_authorization", &"[REDACTED]")
            .field(
                "allow_loopback_specify_http",
                &self.allow_loopback_specify_http,
            )
            .field("heartbeat_interval", &self.heartbeat_interval)
            .field("leadership_lease", &self.leadership_lease)
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum EnterpriseStartupError {
    #[error("invalid enterprise configuration: {0}")]
    Invalid(String),
    #[error("Specify startup check failed: {0}")]
    Specify(String),
    #[error("enterprise coordination failed: {0}")]
    Coordination(String),
}

impl EnterpriseControlPlaneConfig {
    pub fn from_daemon(config: &DaemonConfig) -> Result<Option<Self>, EnterpriseStartupError> {
        if config.security_mode != SecurityRuntimeMode::Enterprise {
            return Ok(None);
        }
        let enterprise = &config.enterprise;
        let instance_id = required(
            enterprise.control_plane_instance_id.as_deref(),
            "control-plane instance id",
        )?;
        if instance_id.len() != 29
            || !instance_id.starts_with("ci_")
            || !instance_id
                .as_bytes()
                .get(3)
                .is_some_and(|byte| matches!(byte, b'0'..=b'7'))
            || instance_id[3..].bytes().any(|byte| {
                !matches!(
                    byte,
                    b'0'..=b'9'
                        | b'A'..=b'H'
                        | b'J'..=b'K'
                        | b'M'..=b'N'
                        | b'P'..=b'T'
                        | b'V'..=b'Z'
                )
            })
        {
            return Err(EnterpriseStartupError::Invalid(
                "control-plane instance id must be ci_<26-character ULID>".to_string(),
            ));
        }
        let region = required(enterprise.enterprise_region.as_deref(), "enterprise region")?;
        let zone = required(enterprise.enterprise_zone.as_deref(), "enterprise zone")?;
        let endpoint = required(
            enterprise.control_plane_endpoint.as_deref(),
            "control-plane endpoint",
        )?;
        let specify_url = required(enterprise.specify_url.as_deref(), "Specify URL")?;
        let authority = required(
            enterprise.specify_authority_key.as_deref(),
            "Specify authority key",
        )?;
        let authorization_file = enterprise
            .specify_authorization_file
            .as_deref()
            .ok_or_else(|| {
                EnterpriseStartupError::Invalid(
                    "Specify workload authorization file is required".to_string(),
                )
            })?;
        let specify_authorization = read_authorization(authorization_file)?;
        if enterprise.control_plane_heartbeat_seconds == 0
            || enterprise.control_plane_heartbeat_seconds > 60
            || enterprise.control_plane_lease_seconds
                <= enterprise.control_plane_heartbeat_seconds * 2
            || enterprise.control_plane_lease_seconds > 300
        {
            return Err(EnterpriseStartupError::Invalid(
                "heartbeat must be 1..=60s and lease must be >2x heartbeat and <=300s".to_string(),
            ));
        }
        Ok(Some(Self {
            instance_id: ControlPlaneInstanceId::from_string(instance_id),
            region,
            zone,
            endpoint_sha256: format!("{:x}", Sha256::digest(endpoint.as_bytes())),
            specify_url,
            specify_authority_key: AuthorityKey::new(authority)
                .map_err(|error| EnterpriseStartupError::Invalid(error.to_string()))?,
            specify_authorization: Zeroizing::new(specify_authorization),
            allow_loopback_specify_http: enterprise.allow_loopback_specify_http,
            heartbeat_interval: Duration::from_secs(enterprise.control_plane_heartbeat_seconds),
            leadership_lease: Duration::from_secs(enterprise.control_plane_lease_seconds),
        }))
    }
}

pub struct EnterpriseCoordinatorHandle {
    leadership_gate: EnterpriseLeadershipGate,
    shutdown_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
}

#[derive(Clone)]
pub struct EnterpriseLeadershipGate {
    instance_id: ControlPlaneInstanceId,
    leadership: Arc<RwLock<Option<ControlPlaneLeadershipLease>>>,
}

impl std::fmt::Debug for EnterpriseLeadershipGate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EnterpriseLeadershipGate")
            .field("instance_id", &self.instance_id)
            .field("leadership", &"[SHARED]")
            .finish()
    }
}

impl EnterpriseLeadershipGate {
    pub async fn authorize_mutation(
        &self,
        observed_at: i64,
    ) -> Result<ControlPlaneLeadershipLease, EnterpriseStartupError> {
        let lease = self.leadership.read().await.clone().ok_or_else(|| {
            EnterpriseStartupError::Coordination(
                "this control-plane instance is not the current leader".to_string(),
            )
        })?;
        if lease.instance_id != self.instance_id || lease.expires_at <= observed_at {
            return Err(EnterpriseStartupError::Coordination(
                "enterprise leadership is stale or expired".to_string(),
            ));
        }
        Ok(lease)
    }

    pub async fn leadership(&self) -> Option<ControlPlaneLeadershipLease> {
        self.leadership.read().await.clone()
    }
}

impl std::fmt::Debug for EnterpriseCoordinatorHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EnterpriseCoordinatorHandle")
            .field("leadership", &"[SHARED]")
            .finish_non_exhaustive()
    }
}

impl EnterpriseCoordinatorHandle {
    pub async fn leadership(&self) -> Option<ControlPlaneLeadershipLease> {
        self.leadership_gate.leadership().await
    }

    #[must_use]
    pub fn leadership_gate(&self) -> EnterpriseLeadershipGate {
        self.leadership_gate.clone()
    }

    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(true);
        let _ = self.task.await;
    }
}

#[allow(clippy::too_many_lines)]
pub async fn start_enterprise_coordination(
    daemon: &DaemonConfig,
    store: &SqliteStore,
) -> Result<Option<EnterpriseCoordinatorHandle>, EnterpriseStartupError> {
    let Some(config) = EnterpriseControlPlaneConfig::from_daemon(daemon)? else {
        return Ok(None);
    };
    let transport = HttpSpecifyAuthorityTransport::new(
        &config.specify_url,
        config.specify_authorization.as_str(),
        Duration::from_secs(10),
        config.allow_loopback_specify_http,
    )
    .map_err(|error| EnterpriseStartupError::Specify(error.to_string()))?;
    let authority = SpecifyProjectAuthority::new(config.specify_authority_key.clone(), transport);
    let health = authority
        .health()
        .await
        .map_err(|error| EnterpriseStartupError::Specify(error.to_string()))?;
    if health.authority_key != config.specify_authority_key
        || health.availability != ProjectAuthorityAvailability::Available
    {
        return Err(EnterpriseStartupError::Specify(
            "Specify health did not confirm the configured authority".to_string(),
        ));
    }
    let scale = Arc::new(SqliteEnterpriseScaleControlPlane::new(store.pool().clone()));
    let now = now_unix()?;
    let started_at = now;
    let snapshot = scale
        .operational_snapshot(now)
        .await
        .map_err(|error| EnterpriseStartupError::Coordination(error.to_string()))?;
    let previous_sequence = snapshot
        .members
        .iter()
        .find(|member| member.instance_id == config.instance_id)
        .map(|member| member.heartbeat_sequence);
    let initial_sequence = previous_sequence.map_or(Ok(1), |sequence| {
        sequence.checked_add(1).ok_or_else(|| {
            EnterpriseStartupError::Coordination(
                "control-plane heartbeat sequence is exhausted".to_string(),
            )
        })
    })?;
    heartbeat(&scale, &config, initial_sequence, started_at, now).await?;
    let leadership = match acquire(&scale, &config, now, initial_sequence).await {
        Ok(lease) => Some(lease),
        Err(EnterpriseScaleError::Denied(_)) => None,
        Err(error) => {
            return Err(EnterpriseStartupError::Coordination(error.to_string()));
        }
    };
    let leadership = Arc::new(RwLock::new(leadership));
    let leadership_gate = EnterpriseLeadershipGate {
        instance_id: config.instance_id.clone(),
        leadership: Arc::clone(&leadership),
    };
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let task_leadership = Arc::clone(&leadership);
    let task = tokio::spawn(async move {
        let mut sequence = initial_sequence;
        let mut interval = tokio::time::interval(config.heartbeat_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }
                _ = interval.tick() => {
                    let Ok(now) = now_unix() else {
                        tracing::error!("enterprise trusted system time is unavailable");
                        continue;
                    };
                    sequence = sequence.saturating_add(1);
                    if let Err(error) = heartbeat(&scale, &config, sequence, started_at, now).await {
                        tracing::error!(%error, "enterprise control-plane heartbeat failed");
                        continue;
                    }
                    let current = task_leadership.read().await.clone();
                    let next = match current {
                        Some(lease) if lease.expires_at > now => {
                            renew(&scale, &config, &lease, now, sequence).await.ok()
                        }
                        _ => acquire(&scale, &config, now, sequence).await.ok(),
                    };
                    *task_leadership.write().await = next;
                }
            }
        }
        if let Ok(now) = now_unix() {
            let _ = heartbeat_with_status(
                &scale,
                &config,
                sequence.saturating_add(1),
                started_at,
                now,
                ControlPlaneMemberStatus::Offline,
            )
            .await;
        }
    });
    Ok(Some(EnterpriseCoordinatorHandle {
        leadership_gate,
        shutdown_tx,
        task,
    }))
}

pub struct SpecifyPolicyRevocation {
    authority: SpecifyProjectAuthority<HttpSpecifyAuthorityTransport>,
}

impl std::fmt::Debug for SpecifyPolicyRevocation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SpecifyPolicyRevocation")
            .field("authority", &"[AUTHENTICATED]")
            .finish()
    }
}

impl SpecifyPolicyRevocation {
    pub fn from_config(
        config: &EnterpriseControlPlaneConfig,
    ) -> Result<Self, EnterpriseStartupError> {
        let transport = HttpSpecifyAuthorityTransport::new(
            &config.specify_url,
            config.specify_authorization.as_str(),
            Duration::from_secs(10),
            config.allow_loopback_specify_http,
        )
        .map_err(|error| EnterpriseStartupError::Specify(error.to_string()))?;
        Ok(Self {
            authority: SpecifyProjectAuthority::new(
                config.specify_authority_key.clone(),
                transport,
            ),
        })
    }
}

#[async_trait::async_trait]
impl PolicyRevocationPort for SpecifyPolicyRevocation {
    async fn check_security_epoch(
        &self,
        request: &SecurityEpochRequest,
    ) -> Result<SecurityEpochStatus, SecurityError> {
        let snapshot = self
            .authority
            .refresh(&request.execution_snapshot_ref)
            .await
            .map_err(|error| SecurityError::Unavailable(error.to_string()))?;
        if snapshot.organization_ref != request.organization_ref
            || snapshot.project_ref != request.project_ref
            || snapshot.snapshot_ref != request.execution_snapshot_ref
        {
            return Err(SecurityError::Denied(
                SecurityDenialReason::SnapshotMismatch,
            ));
        }
        Ok(SecurityEpochStatus {
            checkpoint: request.checkpoint,
            organization_ref: snapshot.organization_ref,
            project_ref: snapshot.project_ref,
            execution_snapshot_ref: snapshot.snapshot_ref,
            current_epoch: snapshot.policy_revocation_epoch,
            observed_at: request.observed_at,
        })
    }
}

async fn heartbeat(
    scale: &SqliteEnterpriseScaleControlPlane,
    config: &EnterpriseControlPlaneConfig,
    sequence: u64,
    started_at: i64,
    observed_at: i64,
) -> Result<ControlPlaneMember, EnterpriseStartupError> {
    heartbeat_with_status(
        scale,
        config,
        sequence,
        started_at,
        observed_at,
        ControlPlaneMemberStatus::Ready,
    )
    .await
}

async fn heartbeat_with_status(
    scale: &SqliteEnterpriseScaleControlPlane,
    config: &EnterpriseControlPlaneConfig,
    sequence: u64,
    started_at: i64,
    observed_at: i64,
    status: ControlPlaneMemberStatus,
) -> Result<ControlPlaneMember, EnterpriseStartupError> {
    scale
        .heartbeat_control_plane(&ControlPlaneHeartbeatRequest {
            idempotency_key: format!("{}:{sequence}:{status:?}", config.instance_id.as_str()),
            member: ControlPlaneMember {
                instance_id: config.instance_id.clone(),
                heartbeat_sequence: sequence,
                region: config.region.clone(),
                zone: config.zone.clone(),
                daemon_version: env!("CARGO_PKG_VERSION").to_string(),
                endpoint_sha256: config.endpoint_sha256.clone(),
                status,
                started_at,
                observed_at,
            },
        })
        .await
        .map_err(|error| EnterpriseStartupError::Coordination(error.to_string()))
}

async fn acquire(
    scale: &SqliteEnterpriseScaleControlPlane,
    config: &EnterpriseControlPlaneConfig,
    observed_at: i64,
    sequence: u64,
) -> Result<ControlPlaneLeadershipLease, EnterpriseScaleError> {
    scale
        .acquire_leadership(&ControlPlaneLeadershipRequest {
            instance_id: config.instance_id.clone(),
            idempotency_key: format!("{}:acquire:{sequence}", config.instance_id.as_str()),
            observed_at,
            expires_at: observed_at.saturating_add(
                i64::try_from(config.leadership_lease.as_secs()).unwrap_or(i64::MAX),
            ),
        })
        .await
}

async fn renew(
    scale: &SqliteEnterpriseScaleControlPlane,
    config: &EnterpriseControlPlaneConfig,
    lease: &ControlPlaneLeadershipLease,
    observed_at: i64,
    sequence: u64,
) -> Result<ControlPlaneLeadershipLease, EnterpriseScaleError> {
    scale
        .renew_leadership(&ControlPlaneLeadershipRenewal {
            instance_id: config.instance_id.clone(),
            idempotency_key: format!("{}:renew:{sequence}", config.instance_id.as_str()),
            term: lease.term,
            fencing_token: lease.fencing_token,
            observed_at,
            expires_at: observed_at.saturating_add(
                i64::try_from(config.leadership_lease.as_secs()).unwrap_or(i64::MAX),
            ),
        })
        .await
}

fn required(value: Option<&str>, field: &str) -> Result<String, EnterpriseStartupError> {
    value
        .filter(|value| {
            *value == value.trim()
                && !value.is_empty()
                && value.len() <= 512
                && !value.chars().any(char::is_control)
        })
        .map(str::to_string)
        .ok_or_else(|| EnterpriseStartupError::Invalid(format!("{field} is required")))
}

fn read_authorization(path: &Path) -> Result<String, EnterpriseStartupError> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| EnterpriseStartupError::Invalid(format!("authorization file: {error}")))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_AUTHORIZATION_BYTES {
        return Err(EnterpriseStartupError::Invalid(
            "Specify authorization file must be a non-empty regular file <=16 KiB".to_string(),
        ));
    }
    let value = std::fs::read_to_string(path)
        .map_err(|error| EnterpriseStartupError::Invalid(format!("authorization file: {error}")))?;
    let value = value.trim();
    if value.is_empty()
        || value
            .chars()
            .any(|character| matches!(character, '\r' | '\n'))
    {
        return Err(EnterpriseStartupError::Invalid(
            "Specify authorization value is invalid".to_string(),
        ));
    }
    Ok(value.to_string())
}

fn now_unix() -> Result<i64, EnterpriseStartupError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| EnterpriseStartupError::Coordination(error.to_string()))?
        .as_secs();
    i64::try_from(seconds)
        .map_err(|_| EnterpriseStartupError::Coordination("system time overflow".to_string()))
}
