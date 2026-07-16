//! Repository ownership proof for the complete AD-E1 code candidate.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("read {}: {error}", path.display());
    })
}

#[test]
fn ad_e1_candidate_assigns_every_security_capability_to_code() {
    let owned_symbols = [
        (
            "crates/agentd-core/src/types/principal.rs",
            &[
                "EnterprisePrincipal",
                "PlacementPolicy",
                "SecurityCheckpoint",
            ][..],
        ),
        (
            "crates/agentd-core/src/ports/principal.rs",
            &["EnterprisePrincipalPort"][..],
        ),
        (
            "crates/agentd-core/src/ports/revocation.rs",
            &["PolicyRevocationPort"][..],
        ),
        (
            "crates/agentd-store/src/principal_repo.rs",
            &["SqliteEnterprisePrincipalRepository"][..],
        ),
        (
            "crates/agentd-security/src/oidc.rs",
            &["OidcAuthenticator", "OidcProviderConfig"][..],
        ),
        (
            "crates/agentd-security/src/matrix_principal.rs",
            &["MatrixPrincipalResolver"][..],
        ),
        (
            "crates/agentd-security/src/redaction.rs",
            &["ContentRedactor", "RedactionLimits"][..],
        ),
        (
            "crates/agentd-security/src/placement.rs",
            &["PlacementPolicyEvaluator"][..],
        ),
        (
            "crates/agentd-security/src/remote_secrets.rs",
            &["SecretBrokerTransport", "RemoteSecretBroker"][..],
        ),
        (
            "crates/agentd-security/src/revocation.rs",
            &["AuthorityRevocationChecker"][..],
        ),
        (
            "crates/agentd-bin/src/security.rs",
            &["check_revocation_checkpoint", "placement_policy"][..],
        ),
    ];

    for (path, symbols) in owned_symbols {
        let source = read(path);
        for symbol in symbols {
            assert!(source.contains(symbol), "{path} does not own {symbol}");
        }
    }
}

#[test]
fn ad_e1_principal_schema_contains_no_credentials_or_policy_authority() {
    let migration = read("crates/agentd-store/migrations/0017_enterprise_principals.sql");
    for table in [
        "enterprise_principals",
        "oidc_principal_bindings",
        "matrix_principal_users",
        "matrix_principal_devices",
        "matrix_principal_appservices",
    ] {
        assert!(migration.contains(table), "missing table {table}");
    }
    for forbidden in [
        "access_token",
        "refresh_token",
        "secret_bytes",
        "private_key",
        "device_key",
        "rbac_policy",
        "quota_policy",
    ] {
        assert!(
            !migration.to_ascii_lowercase().contains(forbidden),
            "principal schema must not own {forbidden}"
        );
    }
}

#[test]
fn ad_e1_docs_record_candidate_and_defer_all_real_acceptance() {
    let roadmap = read("docs/plans/2026-07-09-agentd-native-runtime-roadmap.md");
    let checklist = read("docs/acceptance/ad-e-roadmap-manual-checklist.md");
    let parity = read("docs/parity/agent-chat-capability-map.md");

    assert!(roadmap.contains("code-complete candidate"));
    assert!(roadmap.contains("not an AD-E1 or FSF-2 exit"));
    assert!(!roadmap.contains("AD-E1: PASS"));
    assert!(!roadmap.contains("FSF-2: PASS"));
    let checklist_search = checklist.to_ascii_lowercase();
    for phase in 1..=7 {
        assert!(
            checklist.contains(&format!("## AD-E{phase}")),
            "manual checklist missing AD-E{phase}"
        );
    }
    for evidence in [
        "OIDC",
        "Matrix",
        "secret broker",
        "OCI",
        "cross-tenant",
        "revocation",
        "placement",
    ] {
        assert!(
            checklist_search.contains(&evidence.to_ascii_lowercase()),
            "missing manual evidence {evidence}"
        );
    }
    assert!(parity.contains("enterprise_principal_oidc_matrix"));
    assert!(parity.contains("content_redaction"));
    assert!(parity.contains("placement_revocation"));
}
