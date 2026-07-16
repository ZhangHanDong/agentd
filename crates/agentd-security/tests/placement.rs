use std::collections::BTreeSet;

use agentd_core::types::{
    DataClassification, PlacementCandidate, PlacementPolicy, SecurityDenialReason,
};
use agentd_security::placement::PlacementPolicyEvaluator;

fn policy() -> PlacementPolicy {
    PlacementPolicy {
        data_classification: DataClassification::Restricted,
        allowed_regions: BTreeSet::from(["eu-west-1".to_string()]),
        allowed_worker_trust_domains: BTreeSet::from(["workers.example".to_string()]),
        require_signed_image: true,
        require_dedicated_pool: true,
        egress_profile_id: "restricted-egress-v1".to_string(),
        tenant_cache_namespace: "org-a/project-a".to_string(),
    }
}

fn candidate() -> PlacementCandidate {
    PlacementCandidate {
        supported_data_classifications: BTreeSet::from([DataClassification::Restricted]),
        region: "eu-west-1".to_string(),
        worker_trust_domain: "workers.example".to_string(),
        image_digest: format!("sha256:{}", "a".repeat(64)),
        image_signature_verified: true,
        dedicated_pool: true,
        egress_profile_id: "restricted-egress-v1".to_string(),
        tenant_cache_namespace: "org-a/project-a".to_string(),
    }
}

#[test]
fn placement_accepts_exact_immutable_policy_match() {
    let admission = PlacementPolicyEvaluator::evaluate(&policy(), &candidate())
        .expect("matching worker placement");
    assert_eq!(admission.policy, policy());
    assert_eq!(admission.candidate, candidate());
}

#[test]
fn placement_rejects_each_independent_constraint() {
    let cases = [
        (
            {
                let mut value = candidate();
                value.supported_data_classifications.clear();
                value
            },
            SecurityDenialReason::PlacementClassificationDenied,
        ),
        (
            {
                let mut value = candidate();
                value.region = "us-east-1".to_string();
                value
            },
            SecurityDenialReason::PlacementRegionDenied,
        ),
        (
            {
                let mut value = candidate();
                value.worker_trust_domain = "foreign.example".to_string();
                value
            },
            SecurityDenialReason::PlacementTrustDomainDenied,
        ),
        (
            {
                let mut value = candidate();
                value.image_digest = "latest".to_string();
                value
            },
            SecurityDenialReason::PlacementImageDigestInvalid,
        ),
        (
            {
                let mut value = candidate();
                value.image_signature_verified = false;
                value
            },
            SecurityDenialReason::PlacementImageUnsigned,
        ),
        (
            {
                let mut value = candidate();
                value.dedicated_pool = false;
                value
            },
            SecurityDenialReason::PlacementDedicatedPoolRequired,
        ),
        (
            {
                let mut value = candidate();
                value.egress_profile_id = "open-egress".to_string();
                value
            },
            SecurityDenialReason::PlacementEgressDenied,
        ),
        (
            {
                let mut value = candidate();
                value.tenant_cache_namespace = "org-b/project-b".to_string();
                value
            },
            SecurityDenialReason::PlacementCacheIsolationDenied,
        ),
    ];

    for (candidate, expected) in cases {
        assert_eq!(
            PlacementPolicyEvaluator::evaluate(&policy(), &candidate),
            Err(expected)
        );
    }
}

#[test]
fn placement_rejects_noncanonical_sha256_digest() {
    let mut uppercase = candidate();
    uppercase.image_digest = format!("sha256:{}", "A".repeat(64));
    assert_eq!(
        PlacementPolicyEvaluator::evaluate(&policy(), &uppercase),
        Err(SecurityDenialReason::PlacementImageDigestInvalid)
    );
}
