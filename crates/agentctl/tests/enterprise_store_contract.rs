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
fn p267_roadmap_and_parity_record_schema_without_claiming_protocol() {
    let roadmap = read("docs/plans/2026-07-08-agent-chat-replacement-roadmap.md");
    let parity = read("docs/parity/agent-chat-capability-map.md");
    let migration = read("crates/agentd-store/migrations/0013_enterprise_agent_worker_runtime.sql");

    for table in [
        "agent_profiles",
        "legacy_agent_aliases",
        "workers",
        "worker_incarnations",
        "runtime_sessions",
        "runtime_attempts",
    ] {
        assert!(migration.contains(&format!("CREATE TABLE {table}")));
    }
    for expected in ["P267", "0013_enterprise_agent_worker_runtime.sql"] {
        assert!(roadmap.contains(expected), "roadmap missing {expected}");
        assert!(parity.contains(expected), "parity missing {expected}");
    }

    let identity = parity_row(&parity, "durable_runtime_identity");
    assert!(identity.contains("| partial |"));
    assert!(identity.contains("P267"));
    assert!(identity.contains("P270"));
    assert!(identity.contains("worker protocol/runtime binding"));
    assert!(identity.contains("compatibility cutover"));

    let fleet = parity_row(&parity, "worker_fleet_protocol");
    assert!(fleet.contains("| partial |"));
    assert!(fleet.contains("P267"));
    assert!(fleet.contains("network worker protocol remains pending"));

    let immediate = roadmap
        .split("## Immediate Next Step")
        .nth(1)
        .expect("roadmap Immediate Next Step");
    assert!(immediate.contains("P268"));
    assert!(immediate.contains("artifact/audit model"));
}
