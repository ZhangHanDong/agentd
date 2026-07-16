//! Closed enterprise principal, placement, and revocation values.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{OrganizationRef, ProjectExecutionSnapshotRef, ProjectRef, SecurityDenialReason};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EnterprisePrincipalId(String);

impl EnterprisePrincipalId {
    #[must_use]
    pub fn new() -> Self {
        Self(format!("ep_{}", ulid::Ulid::new()))
    }

    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for EnterprisePrincipalId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    Human,
    Service,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalStatus {
    Active,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnterprisePrincipal {
    pub id: EnterprisePrincipalId,
    pub organization_ref: OrganizationRef,
    pub kind: PrincipalKind,
    pub status: PrincipalStatus,
    pub display_name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub disabled_at: Option<i64>,
}

impl EnterprisePrincipal {
    pub fn ensure_active(&self) -> Result<(), SecurityDenialReason> {
        match self.status {
            PrincipalStatus::Active => Ok(()),
            PrincipalStatus::Disabled => Err(SecurityDenialReason::PrincipalDisabled),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OidcPrincipalResolveRequest {
    pub issuer: String,
    pub subject: String,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixPrincipalResolveRequest {
    pub user_id: String,
    pub homeserver: String,
    pub device_id: Option<String>,
    pub appservice_id: Option<String>,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum EnterpriseAuthentication {
    Oidc {
        issuer: String,
        subject: String,
    },
    Matrix {
        user_id: String,
        homeserver: String,
        device_id: Option<String>,
        appservice_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnterpriseRequestIdentity {
    pub principal: EnterprisePrincipal,
    pub authentication: EnterpriseAuthentication,
    pub authenticated_at: i64,
    pub expires_at: i64,
}

impl EnterpriseRequestIdentity {
    #[must_use]
    pub fn oidc(
        principal: EnterprisePrincipal,
        request: OidcPrincipalResolveRequest,
        expires_at: i64,
    ) -> Self {
        Self {
            principal,
            authentication: EnterpriseAuthentication::Oidc {
                issuer: request.issuer,
                subject: request.subject,
            },
            authenticated_at: request.observed_at,
            expires_at,
        }
    }

    #[must_use]
    pub fn matrix(
        principal: EnterprisePrincipal,
        request: MatrixPrincipalResolveRequest,
        expires_at: i64,
    ) -> Self {
        Self {
            principal,
            authentication: EnterpriseAuthentication::Matrix {
                user_id: request.user_id,
                homeserver: request.homeserver,
                device_id: request.device_id,
                appservice_id: request.appservice_id,
            },
            authenticated_at: request.observed_at,
            expires_at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatrixDeviceStatus {
    Current,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixDeviceBinding {
    pub principal_id: EnterprisePrincipalId,
    pub user_id: String,
    pub device_id: String,
    pub status: MatrixDeviceStatus,
    pub bound_at: i64,
    pub revoked_at: Option<i64>,
}

impl MatrixDeviceBinding {
    pub fn ensure_current(&self) -> Result<(), SecurityDenialReason> {
        match self.status {
            MatrixDeviceStatus::Current => Ok(()),
            MatrixDeviceStatus::Revoked => Err(SecurityDenialReason::MatrixDeviceRevoked),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixTrustPolicy {
    pub trusted_homeservers: BTreeSet<String>,
    pub trusted_appservices: BTreeSet<String>,
}

impl MatrixTrustPolicy {
    pub fn authorize_source(
        &self,
        request: &MatrixPrincipalResolveRequest,
    ) -> Result<(), SecurityDenialReason> {
        if !self.trusted_homeservers.contains(&request.homeserver) {
            return Err(SecurityDenialReason::MatrixHomeserverUntrusted);
        }
        if request
            .appservice_id
            .as_ref()
            .is_some_and(|id| !self.trusted_appservices.contains(id))
        {
            return Err(SecurityDenialReason::MatrixAppserviceUntrusted);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataClassification {
    Public,
    Internal,
    Confidential,
    Restricted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementPolicy {
    pub data_classification: DataClassification,
    pub allowed_regions: BTreeSet<String>,
    pub allowed_worker_trust_domains: BTreeSet<String>,
    pub require_signed_image: bool,
    pub require_dedicated_pool: bool,
    pub egress_profile_id: String,
    pub tenant_cache_namespace: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementCandidate {
    pub supported_data_classifications: BTreeSet<DataClassification>,
    pub region: String,
    pub worker_trust_domain: String,
    pub image_digest: String,
    pub image_signature_verified: bool,
    pub dedicated_pool: bool,
    pub egress_profile_id: String,
    pub tenant_cache_namespace: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementAdmission {
    pub policy: PlacementPolicy,
    pub candidate: PlacementCandidate,
}

impl PlacementPolicy {
    pub fn evaluate(
        &self,
        candidate: &PlacementCandidate,
    ) -> Result<PlacementAdmission, SecurityDenialReason> {
        if !candidate
            .supported_data_classifications
            .contains(&self.data_classification)
        {
            return Err(SecurityDenialReason::PlacementClassificationDenied);
        }
        if !self.allowed_regions.contains(&candidate.region) {
            return Err(SecurityDenialReason::PlacementRegionDenied);
        }
        if !self
            .allowed_worker_trust_domains
            .contains(&candidate.worker_trust_domain)
        {
            return Err(SecurityDenialReason::PlacementTrustDomainDenied);
        }
        if !is_sha256_digest(&candidate.image_digest) {
            return Err(SecurityDenialReason::PlacementImageDigestInvalid);
        }
        if self.require_signed_image && !candidate.image_signature_verified {
            return Err(SecurityDenialReason::PlacementImageUnsigned);
        }
        if self.require_dedicated_pool && !candidate.dedicated_pool {
            return Err(SecurityDenialReason::PlacementDedicatedPoolRequired);
        }
        if self.egress_profile_id != candidate.egress_profile_id {
            return Err(SecurityDenialReason::PlacementEgressDenied);
        }
        if self.tenant_cache_namespace != candidate.tenant_cache_namespace {
            return Err(SecurityDenialReason::PlacementCacheIsolationDenied);
        }
        Ok(PlacementAdmission {
            policy: self.clone(),
            candidate: candidate.clone(),
        })
    }
}

fn is_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityCheckpoint {
    Dispatch,
    LeaseRenewal,
    ArtifactAcceptance,
    Delivery,
    Release,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityEpochRequest {
    pub checkpoint: SecurityCheckpoint,
    pub organization_ref: OrganizationRef,
    pub project_ref: ProjectRef,
    pub execution_snapshot_ref: ProjectExecutionSnapshotRef,
    pub pinned_epoch: u64,
    pub observed_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityEpochStatus {
    pub current_epoch: u64,
    pub observed_at: i64,
}

impl SecurityEpochStatus {
    pub fn validate_pinned_epoch(self, pinned_epoch: u64) -> Result<(), SecurityDenialReason> {
        match self.current_epoch.cmp(&pinned_epoch) {
            std::cmp::Ordering::Equal => Ok(()),
            std::cmp::Ordering::Greater => Err(SecurityDenialReason::PolicyEpochStale),
            std::cmp::Ordering::Less => Err(SecurityDenialReason::PolicyEpochRegressed),
        }
    }
}
