use std::collections::BTreeMap;
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

fn parity_rows(markdown: &str) -> BTreeMap<String, Vec<String>> {
    markdown
        .lines()
        .filter(|line| line.starts_with('|') && !line.starts_with("| ---"))
        .filter_map(|line| {
            let cells: Vec<String> = line
                .trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_string())
                .collect();
            (cells.len() == 7 && cells[0] != "Capability ID").then(|| (cells[0].clone(), cells))
        })
        .collect()
}

fn assert_contains_all(document: &str, label: &str, expected: &[&str]) {
    let normalized = document.split_whitespace().collect::<Vec<_>>().join(" ");
    for value in expected {
        assert!(normalized.contains(value), "{label} missing `{value}`");
    }
}

fn assert_security_parity(parity: &str) {
    let rows = parity_rows(parity);
    for capability in [
        "workload_identity_mtls",
        "tenant_project_authorization",
        "fenced_attempt_capabilities",
        "scoped_secret_broker",
        "execution_sandbox_isolation",
    ] {
        let row = rows
            .get(capability)
            .unwrap_or_else(|| panic!("missing AD-E1 parity row {capability}"));
        assert_eq!(row[4], "partial", "{capability} must remain partial");
        assert_eq!(row[6], "AD-E1", "{capability} must map to AD-E1");
        assert!(
            row[5]
                .to_ascii_lowercase()
                .contains("enterprise replacement gap"),
            "{capability} must not claim an agent-chat authority equivalent"
        );
    }
    for capability in ["native_runtime_process", "native_runtime_session_restore"] {
        let row = &rows[capability];
        assert_eq!(row[4], "partial", "{capability} remains acceptance-gated");
        assert_eq!(row[6], "AD-E5", "{capability} belongs to AD-E5");
        assert!(
            row[5].contains("code candidate"),
            "{capability} must distinguish code from accepted evidence"
        );
    }
    assert_eq!(rows["matrix_bridge"][4], "partial");
    assert_eq!(rows["worker_fleet_protocol"][4], "partial");
    let normalized = parity.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(normalized.contains("agentd cannot be declared a complete replacement"));
    assert!(normalized.contains("deferred acceptance pass and human retirement sign-off"));
}

fn assert_candidate_documents(roadmap: &str, design: &str, spec: &str, composition: &str) {
    assert_contains_all(
        roadmap,
        "roadmap",
        &[
            "AD-E1 minimum baseline candidate",
            "not an AD-E1 or FSF-2 exit",
            "agentd/ad-e1-security-baseline",
            "07120fc",
            "620618c",
            "57415c8",
            "49e8597",
            "c5130d5",
            "0be8baf",
            "368d8f3",
            "revalidates the current lease and action capability immediately before each external side effect",
            "protected-operation composition API",
            "enterprise daemon transport remains incomplete",
            "AD-E0 through AD-E6 now have isolated code candidates",
            "their FSF gates remain open",
            "single deferred verification and operator-sign-off pass",
        ],
    );
    assert_contains_all(
        design,
        "design",
        &[
            "sandbox",
            "workload identity",
            "mTLS",
            "secret broker",
            "tenant/project",
            "lease",
            "fencing",
            "Candidate Verification Evidence",
            "not the complete AD-E1/FSF-2 exit gate",
            "0be8baf",
            "368d8f3",
            "revalidates the current lease and action capability immediately before each external side effect",
            "cargo test --workspace",
            "cargo clippy --workspace --all-targets",
            "14/14",
            "quality score `1.0`",
        ],
    );
    assert_contains_all(
        spec,
        "agent-spec",
        &[
            "candidate-only",
            "0016_execution_security.sql",
            "P272-P275 worker fleet native runtime Matrix OpenFab cutover and scale remain unclaimed",
            "AD-E0 AD-E1 FSF-0 and FSF-2 remain incomplete",
            "current lease and the action capability are revalidated immediately before each external side effect",
        ],
    );
    assert_contains_all(
        composition,
        "enterprise composition",
        &[
            "SecurityRuntimeMode",
            "WorkloadIdentityPort",
            "ExecutionSecurityScopePort",
            "TenantAuthorizationPort",
            "TaskLeasePort",
            "AttemptCapabilityPort",
            "SecretBrokerPort",
            "ExecutionSandboxPort",
            "ExecutionAuditPort",
            "TrustedClock",
            "OpenAuth",
            "AuditOnlyAuth",
            "operation_cancelled",
        ],
    );
}

fn assert_digest_only_migration(migration: &str) {
    for forbidden_column in [
        "\n    token ",
        "\n    secret_",
        "\n    private_key",
        "\n    certificate_private",
        "\n    sandbox_path",
    ] {
        assert!(
            !migration.contains(forbidden_column),
            "migration persists forbidden raw material column `{forbidden_column}`"
        );
    }
    assert!(migration.contains("token_sha256"));
    assert!(migration.contains("fencing_token"));
}

#[test]
fn ad_e1_security_baseline_records_scope_and_remaining_gates() {
    let roadmap = read("docs/plans/2026-07-09-agentd-native-runtime-roadmap.md");
    let parity = read("docs/parity/agent-chat-capability-map.md");
    let design =
        read("docs/superpowers/specs/2026-07-12-ad-e1-minimum-security-baseline-design.md");
    let spec = read("specs/e2e/ad-e1-minimum-security-baseline.spec.md");
    let migration = read("crates/agentd-store/migrations/0016_execution_security.sql");
    let composition = read("crates/agentd-bin/src/security.rs");

    assert_security_parity(&parity);
    assert_candidate_documents(&roadmap, &design, &spec, &composition);
    assert_digest_only_migration(&migration);
}
