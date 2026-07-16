//! Pure placement-policy admission before dispatch and lease renewal.

use agentd_core::types::{
    PlacementAdmission, PlacementCandidate, PlacementPolicy, SecurityDenialReason,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct PlacementPolicyEvaluator;

impl PlacementPolicyEvaluator {
    pub fn evaluate(
        policy: &PlacementPolicy,
        candidate: &PlacementCandidate,
    ) -> Result<PlacementAdmission, SecurityDenialReason> {
        policy.evaluate(candidate)
    }
}
