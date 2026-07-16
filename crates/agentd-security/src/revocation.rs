//! Fail-closed authority adapter for pinned policy-revocation epochs.

use agentd_core::ports::{PolicyRevocationPort, SecurityError};
use agentd_core::types::{SecurityEpochRequest, SecurityEpochStatus};

const MAX_OBSERVATION_AGE_SECONDS: i64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityEpochAuthorityError {
    Unavailable,
    Malformed,
}

#[async_trait::async_trait]
pub trait SecurityEpochAuthority: Send + Sync {
    async fn current_epoch(
        &self,
        request: &SecurityEpochRequest,
    ) -> Result<SecurityEpochStatus, SecurityEpochAuthorityError>;
}

#[derive(Debug)]
pub struct AuthorityRevocationChecker<A> {
    authority: A,
    max_observation_age_seconds: i64,
}

impl<A> AuthorityRevocationChecker<A> {
    pub fn new(authority: A, max_observation_age_seconds: i64) -> Result<Self, SecurityError> {
        if !(1..=MAX_OBSERVATION_AGE_SECONDS).contains(&max_observation_age_seconds) {
            return Err(SecurityError::Invalid(
                "policy revocation observation age is outside the supported bound".to_string(),
            ));
        }
        Ok(Self {
            authority,
            max_observation_age_seconds,
        })
    }

    #[must_use]
    pub const fn authority(&self) -> &A {
        &self.authority
    }
}

#[async_trait::async_trait]
impl<A> PolicyRevocationPort for AuthorityRevocationChecker<A>
where
    A: SecurityEpochAuthority,
{
    async fn check_security_epoch(
        &self,
        request: &SecurityEpochRequest,
    ) -> Result<SecurityEpochStatus, SecurityError> {
        let status = self
            .authority
            .current_epoch(request)
            .await
            .map_err(|error| match error {
                SecurityEpochAuthorityError::Unavailable => SecurityError::Unavailable(
                    "policy revocation authority unavailable".to_string(),
                ),
                SecurityEpochAuthorityError::Malformed => SecurityError::Unavailable(
                    "policy revocation authority returned invalid state".to_string(),
                ),
            })?;
        let age = request
            .observed_at
            .checked_sub(status.observed_at)
            .ok_or_else(|| {
                SecurityError::Unavailable(
                    "policy revocation authority returned invalid state".to_string(),
                )
            })?;
        if age < 0 {
            return Err(SecurityError::Unavailable(
                "policy revocation authority returned invalid state".to_string(),
            ));
        }
        if age > self.max_observation_age_seconds {
            return Err(SecurityError::Unavailable(
                "policy revocation authority observation is stale".to_string(),
            ));
        }
        status
            .validate_request(request)
            .map_err(SecurityError::Denied)?;
        status
            .validate_pinned_epoch(request.pinned_epoch)
            .map_err(SecurityError::Denied)?;
        Ok(status)
    }
}
