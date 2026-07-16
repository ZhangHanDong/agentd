//! Matrix source authentication before enterprise-principal resolution.

use std::sync::Arc;

use agentd_core::ports::{EnterprisePrincipalPort, SecurityError};
use agentd_core::types::{
    EnterpriseAuthentication, EnterpriseRequestIdentity, MatrixPrincipalResolveRequest,
    MatrixTrustPolicy, SecurityDenialReason,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixPrincipalResolverConfig {
    pub trust_policy: MatrixTrustPolicy,
    pub require_human_device: bool,
}

#[derive(Debug)]
pub struct MatrixPrincipalResolver<R> {
    repository: Arc<R>,
    config: MatrixPrincipalResolverConfig,
}

impl<R> MatrixPrincipalResolver<R>
where
    R: EnterprisePrincipalPort,
{
    #[must_use]
    pub fn new(repository: Arc<R>, config: MatrixPrincipalResolverConfig) -> Self {
        Self { repository, config }
    }

    pub async fn resolve(
        &self,
        request: &MatrixPrincipalResolveRequest,
    ) -> Result<EnterpriseRequestIdentity, SecurityError> {
        self.config
            .trust_policy
            .authorize_source(request)
            .map_err(SecurityError::Denied)?;
        validate_user_id(request)?;
        if self.config.require_human_device
            && request.appservice_id.is_none()
            && request.device_id.is_none()
        {
            return Err(SecurityError::Denied(
                SecurityDenialReason::MatrixDeviceRequired,
            ));
        }

        let identity = self.repository.resolve_matrix(request).await?;
        identity
            .principal
            .ensure_active()
            .map_err(SecurityError::Denied)?;
        if identity.authenticated_at != request.observed_at
            || identity.expires_at <= request.observed_at
            || !authentication_matches(&identity, request)
        {
            return Err(SecurityError::Denied(
                SecurityDenialReason::IdentityUntrusted,
            ));
        }
        Ok(identity)
    }
}

fn validate_user_id(request: &MatrixPrincipalResolveRequest) -> Result<(), SecurityError> {
    let Some(body) = request.user_id.strip_prefix('@') else {
        return Err(SecurityError::Denied(
            SecurityDenialReason::IdentityUntrusted,
        ));
    };
    let Some((localpart, homeserver)) = body.split_once(':') else {
        return Err(SecurityError::Denied(
            SecurityDenialReason::IdentityUntrusted,
        ));
    };
    if localpart.is_empty() || homeserver != request.homeserver {
        return Err(SecurityError::Denied(
            SecurityDenialReason::IdentityUntrusted,
        ));
    }
    Ok(())
}

fn authentication_matches(
    identity: &EnterpriseRequestIdentity,
    request: &MatrixPrincipalResolveRequest,
) -> bool {
    match &identity.authentication {
        EnterpriseAuthentication::Matrix {
            user_id,
            homeserver,
            device_id,
            appservice_id,
        } => {
            user_id == &request.user_id
                && homeserver == &request.homeserver
                && device_id == &request.device_id
                && appservice_id == &request.appservice_id
        }
        EnterpriseAuthentication::Oidc { .. } => false,
    }
}
