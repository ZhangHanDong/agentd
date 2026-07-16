//! Enterprise execution-security adapters.

#![warn(clippy::unwrap_used, clippy::panic)]

pub mod identity;
pub mod matrix_principal;
pub mod oidc;
pub mod placement;
pub mod redaction;
pub mod remote_secrets;
pub mod revocation;
pub mod sandbox;
pub mod secrets;
