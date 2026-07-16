//! Enterprise request-principal and authority-epoch resolution.

use crate::types::{
    EnterprisePrincipal, EnterprisePrincipalId, EnterpriseRequestIdentity,
    MatrixPrincipalResolveRequest, OidcPrincipalResolveRequest, SecurityEpochRequest,
    SecurityEpochStatus,
};

use super::SecurityError;

#[async_trait::async_trait]
pub trait EnterprisePrincipalPort: Send + Sync {
    async fn get_principal(
        &self,
        id: &EnterprisePrincipalId,
    ) -> Result<EnterprisePrincipal, SecurityError>;

    async fn resolve_oidc(
        &self,
        request: &OidcPrincipalResolveRequest,
    ) -> Result<EnterpriseRequestIdentity, SecurityError>;

    async fn resolve_matrix(
        &self,
        request: &MatrixPrincipalResolveRequest,
    ) -> Result<EnterpriseRequestIdentity, SecurityError>;

    async fn check_security_epoch(
        &self,
        request: &SecurityEpochRequest,
    ) -> Result<SecurityEpochStatus, SecurityError>;
}
