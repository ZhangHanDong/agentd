//! Matrix source authentication before enterprise-principal resolution.

use std::sync::Arc;

use agentd_core::ports::{
    Clock, EnterprisePrincipalPort, MatrixGatewayDenialReason, MatrixGatewayError,
    MatrixGatewayIdentityPort, MatrixTransportProvenance, SecurityError,
};
use agentd_core::types::{
    EnterpriseAuthentication, EnterpriseRequestIdentity, MatrixPrincipalResolveRequest,
    MatrixTrustPolicy, SecurityDenialReason,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixPrincipalResolverConfig {
    pub trust_policy: MatrixTrustPolicy,
}

#[async_trait::async_trait]
impl<R, C> MatrixGatewayIdentityPort for MatrixPrincipalResolver<R, C>
where
    R: EnterprisePrincipalPort,
    C: Clock,
{
    async fn authenticate_matrix_source(
        &self,
        provenance: &MatrixTransportProvenance,
    ) -> Result<EnterpriseRequestIdentity, MatrixGatewayError> {
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
        self.resolve(&MatrixPrincipalResolveRequest {
            user_id: provenance.authenticated_sender_user_id.clone(),
            homeserver: provenance.homeserver.clone(),
            device_id: provenance.device_id.clone(),
            appservice_id: provenance.authenticated_appservice_id.clone(),
            observed_at: 0,
        })
        .await
        .map_err(|error| match error {
            SecurityError::Denied(_) | SecurityError::Invalid(_) => {
                MatrixGatewayError::Denied(MatrixGatewayDenialReason::PrincipalUnauthorized)
            }
            SecurityError::Unavailable(message) => MatrixGatewayError::Unavailable(message),
        })
    }
}

#[derive(Debug)]
pub struct MatrixPrincipalResolver<R, C> {
    repository: Arc<R>,
    clock: Arc<C>,
    config: MatrixPrincipalResolverConfig,
}

impl<R, C> MatrixPrincipalResolver<R, C>
where
    R: EnterprisePrincipalPort,
    C: Clock,
{
    #[must_use]
    pub fn new(repository: Arc<R>, clock: Arc<C>, config: MatrixPrincipalResolverConfig) -> Self {
        Self {
            repository,
            clock,
            config,
        }
    }

    pub async fn resolve(
        &self,
        request: &MatrixPrincipalResolveRequest,
    ) -> Result<EnterpriseRequestIdentity, SecurityError> {
        let observed_at = self.clock.now_unix();
        if observed_at < 0 {
            return Err(SecurityError::Unavailable(
                "trusted Matrix clock returned invalid time".to_string(),
            ));
        }
        let mut request = request.clone();
        request.observed_at = observed_at;
        self.config
            .trust_policy
            .authorize_source(&request)
            .map_err(SecurityError::Denied)?;
        validate_user_id(&request)?;
        if request.appservice_id.is_none() && request.device_id.is_none() {
            return Err(SecurityError::Denied(
                SecurityDenialReason::MatrixDeviceRequired,
            ));
        }

        let identity = self.repository.resolve_matrix(&request).await?;
        identity
            .principal
            .ensure_active()
            .map_err(SecurityError::Denied)?;
        if identity.authenticated_at != request.observed_at
            || identity.expires_at <= request.observed_at
            || !authentication_matches(&identity, &request)
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
