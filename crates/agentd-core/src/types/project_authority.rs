//! Immutable foreign project-authority references and execution snapshots.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::PlacementPolicy;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuthorityKey(String);

impl AuthorityKey {
    pub fn new(value: impl Into<String>) -> Result<Self, ProjectAuthorityValidationError> {
        let value = value.into();
        validate_text(&value, "authority key")?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AuthorityKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Organization,
    Team,
    Project,
    Repository,
    ProjectRoomBinding,
    Issue,
    Requirement,
    FrozenSpec,
    ProductWorkflow,
    RbacPolicy,
    QuotaPolicy,
    CertificationPolicy,
    MatrixRoom,
    ExecutionSnapshot,
}

impl ResourceKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Organization => "organization",
            Self::Team => "team",
            Self::Project => "project",
            Self::Repository => "repository",
            Self::ProjectRoomBinding => "project_room_binding",
            Self::Issue => "issue",
            Self::Requirement => "requirement",
            Self::FrozenSpec => "frozen_spec",
            Self::ProductWorkflow => "product_workflow",
            Self::RbacPolicy => "rbac_policy",
            Self::QuotaPolicy => "quota_policy",
            Self::CertificationPolicy => "certification_policy",
            Self::MatrixRoom => "matrix_room",
            Self::ExecutionSnapshot => "execution_snapshot",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AuthorityResourceRef {
    authority_key: AuthorityKey,
    resource_kind: ResourceKind,
    resource_id: String,
    resource_version: String,
}

impl AuthorityResourceRef {
    pub fn new(
        authority_key: AuthorityKey,
        resource_kind: ResourceKind,
        resource_id: impl Into<String>,
        resource_version: impl Into<String>,
    ) -> Result<Self, ProjectAuthorityValidationError> {
        let resource_id = resource_id.into();
        let resource_version = resource_version.into();
        validate_text(&resource_id, "resource id")?;
        validate_text(&resource_version, "resource version")?;
        if resource_id.eq_ignore_ascii_case("latest")
            || resource_version.eq_ignore_ascii_case("latest")
        {
            return Err(ProjectAuthorityValidationError::MutableReference);
        }
        Ok(Self {
            authority_key,
            resource_kind,
            resource_id,
            resource_version,
        })
    }

    #[must_use]
    pub const fn authority_key(&self) -> &AuthorityKey {
        &self.authority_key
    }

    #[must_use]
    pub const fn resource_kind(&self) -> ResourceKind {
        self.resource_kind
    }

    #[must_use]
    pub fn resource_id(&self) -> &str {
        &self.resource_id
    }

    #[must_use]
    pub fn resource_version(&self) -> &str {
        &self.resource_version
    }
}

macro_rules! typed_authority_ref {
    ($name:ident, $kind:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(AuthorityResourceRef);

        impl $name {
            pub fn new(
                authority_key: AuthorityKey,
                resource_id: impl Into<String>,
                resource_version: impl Into<String>,
            ) -> Result<Self, ProjectAuthorityValidationError> {
                Ok(Self(AuthorityResourceRef::new(
                    authority_key,
                    ResourceKind::$kind,
                    resource_id,
                    resource_version,
                )?))
            }

            #[must_use]
            pub const fn as_resource_ref(&self) -> &AuthorityResourceRef {
                &self.0
            }

            #[must_use]
            pub const fn authority_key(&self) -> &AuthorityKey {
                self.0.authority_key()
            }

            #[must_use]
            pub fn resource_id(&self) -> &str {
                self.0.resource_id()
            }

            #[must_use]
            pub fn resource_version(&self) -> &str {
                self.0.resource_version()
            }
        }

        impl TryFrom<AuthorityResourceRef> for $name {
            type Error = ProjectAuthorityValidationError;

            fn try_from(value: AuthorityResourceRef) -> Result<Self, Self::Error> {
                if value.resource_kind() != ResourceKind::$kind {
                    return Err(ProjectAuthorityValidationError::WrongResourceKind {
                        expected: ResourceKind::$kind,
                        actual: value.resource_kind(),
                    });
                }
                Ok(Self(value))
            }
        }

        impl From<$name> for AuthorityResourceRef {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

typed_authority_ref!(OrganizationRef, Organization);
typed_authority_ref!(TeamRef, Team);
typed_authority_ref!(ProjectRef, Project);
typed_authority_ref!(RepositoryRef, Repository);
typed_authority_ref!(ProjectRoomBindingRef, ProjectRoomBinding);
typed_authority_ref!(IssueRef, Issue);
typed_authority_ref!(RequirementRef, Requirement);
typed_authority_ref!(FrozenSpecVersionRef, FrozenSpec);
typed_authority_ref!(ProductWorkflowRef, ProductWorkflow);
typed_authority_ref!(RbacPolicyVersionRef, RbacPolicy);
typed_authority_ref!(QuotaPolicyVersionRef, QuotaPolicy);
typed_authority_ref!(CertificationPolicyVersionRef, CertificationPolicy);
typed_authority_ref!(MatrixRoomRef, MatrixRoom);
typed_authority_ref!(ProjectExecutionSnapshotRef, ExecutionSnapshot);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryRole {
    Target,
    Source,
    Dependency,
    Docs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryBinding {
    pub repository_ref: RepositoryRef,
    pub role: RepositoryRole,
    pub forge_locator: Option<String>,
    pub base_commit: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomBindingRole {
    Command,
    Notification,
    Review,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomBinding {
    pub binding_ref: ProjectRoomBindingRef,
    pub project_ref: ProjectRef,
    pub matrix_room_ref: MatrixRoomRef,
    pub roles: Vec<RoomBindingRole>,
    pub allowed_command_classes: Vec<String>,
    pub rbac_policy_version_ref: RbacPolicyVersionRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfflineRecoveryPolicy {
    Deny,
    AllowPinnedUntilExpiry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectExecutionSnapshot {
    pub snapshot_ref: ProjectExecutionSnapshotRef,
    pub authority_key: AuthorityKey,
    pub authority_revision: u64,
    pub organization_ref: OrganizationRef,
    pub team_refs: Vec<TeamRef>,
    pub project_ref: ProjectRef,
    pub repository_bindings: Vec<RepositoryBinding>,
    pub room_bindings: Vec<RoomBinding>,
    pub issue_ref: Option<IssueRef>,
    pub requirement_refs: Vec<RequirementRef>,
    pub frozen_spec_version_ref: FrozenSpecVersionRef,
    pub product_workflow_ref: ProductWorkflowRef,
    pub rbac_policy_version_ref: RbacPolicyVersionRef,
    pub quota_policy_version_ref: QuotaPolicyVersionRef,
    pub certification_policy_version_ref: Option<CertificationPolicyVersionRef>,
    pub placement_policy: PlacementPolicy,
    pub policy_revocation_epoch: u64,
    pub issued_at: i64,
    pub valid_until: i64,
    pub content_sha256: String,
    pub offline_recovery_policy: OfflineRecoveryPolicy,
}

impl ProjectExecutionSnapshot {
    pub fn validate(&self) -> Result<(), ProjectAuthorityValidationError> {
        if self.authority_revision == 0 {
            return Err(ProjectAuthorityValidationError::InvalidAuthorityRevision);
        }
        if self.issued_at >= self.valid_until {
            return Err(ProjectAuthorityValidationError::InvalidValidityWindow);
        }
        if self.policy_revocation_epoch == 0 {
            return Err(ProjectAuthorityValidationError::InvalidPolicyRevocationEpoch);
        }
        if self.placement_policy.allowed_regions.is_empty()
            || self
                .placement_policy
                .allowed_worker_trust_domains
                .is_empty()
            || self.placement_policy.egress_profile_id.trim().is_empty()
            || self
                .placement_policy
                .tenant_cache_namespace
                .trim()
                .is_empty()
        {
            return Err(ProjectAuthorityValidationError::InvalidPlacementPolicy);
        }
        validate_sha256(&self.content_sha256, "snapshot content sha256")?;

        for reference in self.authority_owned_refs() {
            if reference.authority_key() != &self.authority_key {
                return Err(ProjectAuthorityValidationError::AuthorityMismatch {
                    expected: self.authority_key.clone(),
                    actual: reference.authority_key().clone(),
                });
            }
        }

        if self.repository_bindings.is_empty() {
            return Err(ProjectAuthorityValidationError::MissingRepositoryBinding);
        }
        let mut repositories = HashSet::new();
        for binding in &self.repository_bindings {
            if !repositories.insert(binding.repository_ref.clone()) {
                return Err(ProjectAuthorityValidationError::DuplicateRepositoryBinding);
            }
            validate_commit(&binding.base_commit)?;
        }
        self.target_repository()?;

        let mut command_rooms = HashSet::new();
        for binding in &self.room_bindings {
            if binding.project_ref != self.project_ref {
                return Err(ProjectAuthorityValidationError::RoomProjectMismatch);
            }
            if binding.roles.is_empty() {
                return Err(ProjectAuthorityValidationError::EmptyRoomRoles);
            }
            if binding.roles.contains(&RoomBindingRole::Command) {
                if binding.allowed_command_classes.is_empty() {
                    return Err(ProjectAuthorityValidationError::EmptyCommandClasses);
                }
                if !command_rooms.insert(binding.matrix_room_ref.clone()) {
                    return Err(ProjectAuthorityValidationError::DuplicateCommandRoom);
                }
            }
            for command_class in &binding.allowed_command_classes {
                validate_text(command_class, "allowed command class")?;
            }
        }
        Ok(())
    }

    pub fn target_repository(&self) -> Result<&RepositoryBinding, ProjectAuthorityValidationError> {
        let mut targets = self
            .repository_bindings
            .iter()
            .filter(|binding| binding.role == RepositoryRole::Target);
        let target = targets
            .next()
            .ok_or(ProjectAuthorityValidationError::TargetRepositoryCount(0))?;
        if targets.next().is_some() {
            let count = self
                .repository_bindings
                .iter()
                .filter(|binding| binding.role == RepositoryRole::Target)
                .count();
            return Err(ProjectAuthorityValidationError::TargetRepositoryCount(
                count,
            ));
        }
        Ok(target)
    }

    fn authority_owned_refs(&self) -> Vec<&AuthorityResourceRef> {
        let mut refs = vec![
            self.snapshot_ref.as_resource_ref(),
            self.organization_ref.as_resource_ref(),
            self.project_ref.as_resource_ref(),
            self.frozen_spec_version_ref.as_resource_ref(),
            self.product_workflow_ref.as_resource_ref(),
            self.rbac_policy_version_ref.as_resource_ref(),
            self.quota_policy_version_ref.as_resource_ref(),
        ];
        refs.extend(self.team_refs.iter().map(TeamRef::as_resource_ref));
        refs.extend(
            self.repository_bindings
                .iter()
                .map(|binding| binding.repository_ref.as_resource_ref()),
        );
        for binding in &self.room_bindings {
            refs.push(binding.binding_ref.as_resource_ref());
            refs.push(binding.project_ref.as_resource_ref());
            refs.push(binding.rbac_policy_version_ref.as_resource_ref());
        }
        if let Some(issue_ref) = &self.issue_ref {
            refs.push(issue_ref.as_resource_ref());
        }
        refs.extend(
            self.requirement_refs
                .iter()
                .map(RequirementRef::as_resource_ref),
        );
        if let Some(certification_ref) = &self.certification_policy_version_ref {
            refs.push(certification_ref.as_resource_ref());
        }
        refs
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProjectAuthorityValidationError {
    #[error("{0} must not be empty")]
    Empty(&'static str),
    #[error("authority references must be immutable and cannot use latest")]
    MutableReference,
    #[error("wrong resource kind: expected {expected:?}, got {actual:?}")]
    WrongResourceKind {
        expected: ResourceKind,
        actual: ResourceKind,
    },
    #[error("authority mismatch: expected {expected}, got {actual}")]
    AuthorityMismatch {
        expected: AuthorityKey,
        actual: AuthorityKey,
    },
    #[error("authority revision must be positive")]
    InvalidAuthorityRevision,
    #[error("snapshot validity window must have issued_at before valid_until")]
    InvalidValidityWindow,
    #[error("policy revocation epoch must be positive")]
    InvalidPolicyRevocationEpoch,
    #[error("placement policy must declare region, trust-domain, egress, and cache constraints")]
    InvalidPlacementPolicy,
    #[error("{0} must be 64 lowercase hexadecimal characters")]
    InvalidSha256(&'static str),
    #[error("repository base commit must be 40 lowercase hexadecimal characters")]
    InvalidBaseCommit,
    #[error("at least one repository binding is required")]
    MissingRepositoryBinding,
    #[error("repository bindings must be unique")]
    DuplicateRepositoryBinding,
    #[error("snapshot must contain exactly one target repository, got {0}")]
    TargetRepositoryCount(usize),
    #[error("room binding project does not match snapshot project")]
    RoomProjectMismatch,
    #[error("room binding roles must not be empty")]
    EmptyRoomRoles,
    #[error("command room binding must declare allowed command classes")]
    EmptyCommandClasses,
    #[error("a Matrix room may have at most one command binding per snapshot")]
    DuplicateCommandRoom,
}

fn validate_text(value: &str, field: &'static str) -> Result<(), ProjectAuthorityValidationError> {
    if value.trim().is_empty() {
        return Err(ProjectAuthorityValidationError::Empty(field));
    }
    Ok(())
}

fn validate_sha256(
    value: &str,
    field: &'static str,
) -> Result<(), ProjectAuthorityValidationError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(ProjectAuthorityValidationError::InvalidSha256(field));
    }
    Ok(())
}

fn validate_commit(value: &str) -> Result<(), ProjectAuthorityValidationError> {
    if value.len() != 40
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(ProjectAuthorityValidationError::InvalidBaseCommit);
    }
    Ok(())
}
