//! Current authority policy-epoch checks at protected lifecycle boundaries.

use crate::types::{SecurityEpochRequest, SecurityEpochStatus};

use super::SecurityError;

#[async_trait::async_trait]
pub trait PolicyRevocationPort: Send + Sync {
    async fn check_security_epoch(
        &self,
        request: &SecurityEpochRequest,
    ) -> Result<SecurityEpochStatus, SecurityError>;
}
