//! Pinned-key OIDC authentication for enterprise human and API principals.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use agentd_core::ports::{Clock, EnterprisePrincipalPort, SecurityError};
use agentd_core::types::{
    EnterpriseAuthentication, EnterpriseRequestIdentity, OidcPrincipalResolveRequest,
    SecurityDenialReason,
};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OidcSigningAlgorithm {
    Rs256,
}

impl OidcSigningAlgorithm {
    const fn jsonwebtoken(self) -> Algorithm {
        match self {
            Self::Rs256 => Algorithm::RS256,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OidcJwk {
    pub kid: String,
    pub algorithm: OidcSigningAlgorithm,
    pub modulus_base64url: String,
    pub exponent_base64url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OidcProviderConfig {
    pub issuer: String,
    pub audiences: BTreeSet<String>,
    pub authorized_parties: BTreeSet<String>,
    pub keys: Vec<OidcJwk>,
}

struct PreparedJwk {
    algorithm: OidcSigningAlgorithm,
    decoding_key: DecodingKey,
}

pub struct OidcAuthenticator<R, C> {
    repository: Arc<R>,
    clock: Arc<C>,
    config: OidcProviderConfig,
    keys: BTreeMap<String, PreparedJwk>,
}

impl<R, C> std::fmt::Debug for OidcAuthenticator<R, C> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OidcAuthenticator")
            .field("issuer", &self.config.issuer)
            .field("audiences", &self.config.audiences)
            .field("authorized_parties", &self.config.authorized_parties)
            .field("key_ids", &self.keys.keys().collect::<Vec<_>>())
            .field("repository", &std::any::type_name::<R>())
            .field("clock", &std::any::type_name::<C>())
            .finish()
    }
}

impl<R, C> OidcAuthenticator<R, C>
where
    R: EnterprisePrincipalPort,
    C: Clock,
{
    pub fn new(
        repository: Arc<R>,
        clock: Arc<C>,
        config: OidcProviderConfig,
    ) -> Result<Self, SecurityError> {
        validate_config(&config)?;
        let mut keys = BTreeMap::new();
        for jwk in &config.keys {
            let decoding_key =
                DecodingKey::from_rsa_components(&jwk.modulus_base64url, &jwk.exponent_base64url)
                    .map_err(|_| SecurityError::Invalid("OIDC JWK is invalid".to_string()))?;
            if keys
                .insert(
                    jwk.kid.clone(),
                    PreparedJwk {
                        algorithm: jwk.algorithm,
                        decoding_key,
                    },
                )
                .is_some()
            {
                return Err(SecurityError::Invalid(
                    "OIDC JWK kid is duplicated".to_string(),
                ));
            }
        }
        Ok(Self {
            repository,
            clock,
            config,
            keys,
        })
    }

    #[must_use]
    pub fn repository(&self) -> &R {
        self.repository.as_ref()
    }

    pub async fn authenticate(
        &self,
        token: &str,
    ) -> Result<EnterpriseRequestIdentity, SecurityError> {
        let observed_at = self.clock.now_unix();
        if observed_at < 0 {
            return Err(SecurityError::Unavailable(
                "trusted OIDC clock returned invalid time".to_string(),
            ));
        }
        if token.is_empty() {
            return Err(untrusted());
        }
        let header = jsonwebtoken::decode_header(token).map_err(|_| untrusted())?;
        let kid = header.kid.as_deref().ok_or_else(untrusted)?;
        let key = self.keys.get(kid).ok_or_else(untrusted)?;
        if header.alg != key.algorithm.jsonwebtoken() {
            return Err(untrusted());
        }

        let mut validation = Validation::new(key.algorithm.jsonwebtoken());
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.validate_aud = false;
        validation.required_spec_claims.clear();
        let claims = jsonwebtoken::decode::<OidcClaims>(token, &key.decoding_key, &validation)
            .map_err(|_| untrusted())?
            .claims;
        self.validate_claims(&claims, observed_at)?;

        let mut identity = self
            .repository
            .resolve_oidc(&OidcPrincipalResolveRequest {
                issuer: claims.iss.clone(),
                subject: claims.sub.clone(),
                observed_at,
            })
            .await?;
        identity
            .principal
            .ensure_active()
            .map_err(SecurityError::Denied)?;
        if identity.authenticated_at != observed_at
            || !matches!(
                &identity.authentication,
                EnterpriseAuthentication::Oidc { issuer, subject }
                    if issuer == &claims.iss && subject == &claims.sub
            )
        {
            return Err(untrusted());
        }
        identity.expires_at = identity.expires_at.min(claims.exp);
        if identity.expires_at <= observed_at {
            return Err(SecurityError::Denied(SecurityDenialReason::IdentityExpired));
        }
        Ok(identity)
    }

    fn validate_claims(&self, claims: &OidcClaims, observed_at: i64) -> Result<(), SecurityError> {
        if claims.iss != self.config.issuer
            || claims.sub.trim().is_empty()
            || !claims.aud.matches(&self.config.audiences)
            || !claims
                .aud
                .authorized_party_is_valid(claims.azp.as_deref(), &self.config.authorized_parties)
            || claims
                .nbf
                .is_some_and(|not_before| not_before > observed_at)
            || claims.iat.is_some_and(|issued_at| issued_at > observed_at)
        {
            return Err(untrusted());
        }
        if claims.exp <= observed_at {
            return Err(SecurityError::Denied(SecurityDenialReason::IdentityExpired));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct OidcClaims {
    iss: String,
    sub: String,
    aud: AudienceClaim,
    exp: i64,
    nbf: Option<i64>,
    iat: Option<i64>,
    azp: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AudienceClaim {
    One(String),
    Many(Vec<String>),
}

impl AudienceClaim {
    fn matches(&self, configured: &BTreeSet<String>) -> bool {
        match self {
            Self::One(audience) => configured.contains(audience),
            Self::Many(audiences) => audiences
                .iter()
                .any(|audience| configured.contains(audience)),
        }
    }

    fn authorized_party_is_valid(
        &self,
        authorized_party: Option<&str>,
        configured: &BTreeSet<String>,
    ) -> bool {
        let audiences = match self {
            Self::One(audience) => std::slice::from_ref(audience),
            Self::Many(audiences) => audiences.as_slice(),
        };
        let distinct_count = audiences.iter().collect::<BTreeSet<_>>().len();
        if distinct_count > 1 && authorized_party.is_none() {
            return false;
        }
        authorized_party.is_none_or(|party| {
            configured.contains(party) && audiences.iter().any(|audience| audience == party)
        })
    }
}

fn validate_config(config: &OidcProviderConfig) -> Result<(), SecurityError> {
    let issuer = Url::parse(&config.issuer)
        .map_err(|_| SecurityError::Invalid("OIDC issuer is invalid".to_string()))?;
    if issuer.scheme() != "https"
        || issuer.host_str().is_none()
        || !issuer.username().is_empty()
        || issuer.password().is_some()
        || !issuer.query_pairs().collect::<Vec<_>>().is_empty()
        || issuer.fragment().is_some()
    {
        return Err(SecurityError::Invalid(
            "OIDC issuer must be an HTTPS origin or path without credentials, query, or fragment"
                .to_string(),
        ));
    }
    if config.audiences.is_empty()
        || config
            .audiences
            .iter()
            .any(|audience| audience.trim().is_empty())
        || config.keys.is_empty()
    {
        return Err(SecurityError::Invalid(
            "OIDC audiences and keys must be non-empty".to_string(),
        ));
    }
    for key in &config.keys {
        if key.kid.trim().is_empty()
            || key.modulus_base64url.trim().is_empty()
            || key.exponent_base64url.trim().is_empty()
        {
            return Err(SecurityError::Invalid(
                "OIDC JWK fields must be non-empty".to_string(),
            ));
        }
    }
    Ok(())
}

fn untrusted() -> SecurityError {
    SecurityError::Denied(SecurityDenialReason::IdentityUntrusted)
}
