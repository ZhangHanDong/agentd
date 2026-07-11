//! Explicit standalone and enterprise project-authority adapters.

#![doc(html_root_url = "https://docs.rs/agentd-project-authority/0.0.0")]
#![warn(clippy::unwrap_used, clippy::panic)]

mod control_plane;
mod local;
mod specify;

pub use control_plane::{
    PinnedProjectSnapshot, ProjectAuthorityControlPlane, RecoveryAuthorization, RecoveryInputs,
};
pub use local::LocalProjectAuthority;
pub use specify::{SpecifyAuthorityTransport, SpecifyProjectAuthority};
