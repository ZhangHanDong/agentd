use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn parity_row<'a>(map: &'a str, id: &str) -> &'a str {
    map.lines()
        .find(|line| {
            line.strip_prefix('|')
                .and_then(|rest| rest.split('|').next())
                .map(str::trim)
                .is_some_and(|cell| cell.trim_matches('`') == id)
        })
        .unwrap_or_else(|| panic!("missing parity row {id}"))
}

#[test]
fn p271_roadmap_and_parity_record_evidence_apis_without_claiming_upload_or_openfab_network() {
    let roadmap = read("docs/plans/2026-07-08-agent-chat-replacement-roadmap.md");
    let parity = read("docs/parity/agent-chat-capability-map.md");
    let design = read("docs/specs/2026-07-10-control-plane-execution-evidence-api.md");
    let ports = read("crates/agentd-core/src/ports/execution_evidence.rs");
    let adapter = read("crates/agentd-store/src/execution_evidence_control_plane.rs");

    for expected in [
        "ArtifactIndexPort",
        "ExecutionAuditPort",
        "UsageLedgerPort",
        "CertificationReferencePort",
        "usage.measured",
        "execution.report_rejected",
        "gate=none",
        "P279",
    ] {
        assert!(design.contains(expected), "design missing {expected}");
    }
    for port in [
        "trait ArtifactIndexPort",
        "trait ExecutionAuditPort",
        "trait UsageLedgerPort",
        "trait CertificationReferencePort",
    ] {
        assert!(ports.contains(port), "core API missing {port}");
    }
    for expected in [
        "SqliteExecutionEvidenceControlPlane",
        "usage.measured",
        "execution.report_rejected",
        "validate_claim",
    ] {
        assert!(adapter.contains(expected), "adapter missing {expected}");
    }
    assert!(
        !repo_root()
            .join("crates/agentd-store/migrations/0016_execution_usage.sql")
            .exists(),
        "P271 must reuse the P268 audit table"
    );

    for expected in [
        "P271",
        "ArtifactIndexPort",
        "ExecutionAuditPort",
        "UsageLedgerPort",
        "CertificationReferencePort",
    ] {
        assert!(roadmap.contains(expected), "roadmap missing {expected}");
        assert!(parity.contains(expected), "parity missing {expected}");
    }
    let evidence = parity_row(&parity, "artifact_audit_provenance");
    assert!(evidence.contains("| partial |"));
    for pending in ["object storage", "OpenFab network", "cutover"] {
        assert!(evidence.contains(pending), "evidence row missing {pending}");
    }
    let policy = parity_row(&parity, "auth_rbac_quota");
    assert!(policy.contains("| partial |"));
    assert!(policy.contains("P271"));
    assert!(policy.contains("P279 enforcement"));
    let leases = parity_row(&parity, "durable_task_leases");
    assert!(leases.contains("P271"));
    assert!(leases.contains("rejection audit"));

    let immediate = roadmap
        .split("## Immediate Next Step")
        .nth(1)
        .expect("roadmap Immediate Next Step");
    assert!(immediate.contains("p272-runtime-compatibility-port.spec.md"));
    for expected in ["status", "capture", "shutdown", "rebind"] {
        assert!(immediate.contains(expected), "next step missing {expected}");
    }
}
