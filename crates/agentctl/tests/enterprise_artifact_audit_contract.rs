use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
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
fn p268_roadmap_and_parity_record_store_without_claiming_integration() {
    let roadmap = read("docs/plans/2026-07-08-agent-chat-replacement-roadmap.md");
    let parity = read("docs/parity/agent-chat-capability-map.md");
    let migration = read("crates/agentd-store/migrations/0014_enterprise_artifact_audit.sql");

    for table in [
        "execution_artifacts",
        "legacy_artifact_mappings",
        "artifact_certification_refs",
        "execution_audit_events",
    ] {
        assert!(
            migration.contains(&format!("CREATE TABLE {table}")),
            "migration must create {table}"
        );
    }
    for expected in ["P268", "0014_enterprise_artifact_audit.sql"] {
        assert!(roadmap.contains(expected), "roadmap missing {expected}");
        assert!(parity.contains(expected), "parity missing {expected}");
    }

    let artifact_audit = parity_row(&parity, "artifact_audit_provenance");
    assert!(artifact_audit.contains("| partial |"));
    assert!(artifact_audit.contains("P268"));
    assert!(artifact_audit.contains("P271"));
    assert!(artifact_audit.contains("object storage"));
    assert!(artifact_audit.contains("OpenFab network"));
    assert!(artifact_audit.contains("cutover"));

    let immediate = roadmap
        .split("## Immediate Next Step")
        .nth(1)
        .expect("roadmap Immediate Next Step");
    assert!(immediate.contains("p268-enterprise-artifact-audit-model.spec.md"));
    assert!(immediate.contains("0014_enterprise_artifact_audit.sql"));
    assert!(immediate.contains("p269-control-plane-project-api.spec.md"));
    assert!(immediate.contains("control-plane project API"));
}
