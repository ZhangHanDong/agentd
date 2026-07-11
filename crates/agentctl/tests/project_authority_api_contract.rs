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
fn p269_roadmap_and_parity_record_api_without_claiming_network_integration() {
    let roadmap = read("docs/plans/2026-07-08-agent-chat-replacement-roadmap.md");
    let parity = read("docs/parity/agent-chat-capability-map.md");
    let design = read("docs/specs/2026-07-10-project-authority-port-api.md");
    let port = read("crates/agentd-core/src/ports/project_authority.rs");
    let adapters = read("crates/agentd-project-authority/src/lib.rs");

    for expected in [
        "ProjectAuthorityPort",
        "LocalProjectAuthority",
        "SpecifyProjectAuthority",
        "fail-closed",
    ] {
        assert!(design.contains(expected), "design missing {expected}");
    }
    for method in ["async fn resolve", "async fn refresh", "async fn health"] {
        assert!(port.contains(method), "port missing {method}");
    }
    for adapter in ["LocalProjectAuthority", "SpecifyProjectAuthority"] {
        assert!(adapters.contains(adapter), "crate export missing {adapter}");
    }
    for expected in ["P269", "ProjectAuthorityPort", "LocalProjectAuthority"] {
        assert!(roadmap.contains(expected), "roadmap missing {expected}");
        assert!(parity.contains(expected), "parity missing {expected}");
    }
    assert!(roadmap.contains("SpecifyProjectAuthority"));
    assert!(parity.contains("SpecifyProjectAuthority"));

    let binding = parity_row(&parity, "project_room_repo_binding");
    assert!(binding.contains("| partial |"));
    assert!(binding.contains("P269"));
    assert!(binding.contains("Specify network and durable pinning integration remain pending"));

    let immediate = roadmap
        .split("## Immediate Next Step")
        .nth(1)
        .expect("roadmap Immediate Next Step");
    assert!(immediate.contains("p269-control-plane-project-api.spec.md"));
    assert!(immediate.contains("ProjectAuthorityPort"));
    assert!(immediate.contains("p270-control-plane-dispatch-api.spec.md"));
    for expected in ["dispatch", "lease", "fencing"] {
        assert!(immediate.contains(expected), "next step missing {expected}");
    }
}
