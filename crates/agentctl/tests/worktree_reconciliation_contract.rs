use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

fn table_rows(markdown: &str) -> Vec<Vec<String>> {
    markdown
        .lines()
        .filter(|line| line.starts_with("| p"))
        .map(|line| {
            line.trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_string())
                .collect()
        })
        .collect()
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

fn e2e_spec_ids(spec_dir: &Path) -> Vec<u32> {
    let mut ids = Vec::new();
    for entry in std::fs::read_dir(spec_dir).expect("read e2e specs") {
        let entry = entry.expect("spec entry");
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(rest) = name.strip_prefix('p') else {
            continue;
        };
        let Some((number, _)) = rest.split_once('-') else {
            continue;
        };
        if let Ok(number) = number.parse::<u32>()
            && number >= 200
        {
            ids.push(number);
        }
    }
    ids.sort_unstable();
    ids
}

#[test]
fn p263_reconciliation_maps_every_conflicting_source_spec() {
    let manifest = read("docs/parity/agent-chat-worktree-reconciliation.md");
    let rows = table_rows(&manifest);
    let source_ids: Vec<String> = rows.iter().map(|row| row[0].clone()).collect();
    let expected: Vec<String> = (202..=228).map(|id| format!("p{id}")).collect();
    assert_eq!(source_ids, expected);

    let port_required: BTreeSet<String> = rows
        .iter()
        .filter(|row| row[3] == "port_required")
        .map(|row| row[0].clone())
        .collect();
    assert_eq!(
        port_required,
        ["p205", "p210", "p211", "p219", "p220"]
            .into_iter()
            .map(str::to_string)
            .collect()
    );

    let destinations: BTreeMap<&str, &str> = rows
        .iter()
        .filter(|row| (223..=228).contains(&row[0][1..].parse::<u32>().expect("source id")))
        .map(|row| (row[0].as_str(), row[4].as_str()))
        .collect();
    for (source, destination) in [
        ("p223", "p263"),
        ("p224", "p264"),
        ("p225", "p265"),
        ("p226", "p266"),
        ("p227", "p267"),
        ("p228", "p268"),
    ] {
        assert_eq!(destinations.get(source), Some(&destination));
    }
}

#[test]
fn p263_reconciliation_reserves_non_conflicting_sequences() {
    let manifest = read("docs/parity/agent-chat-worktree-reconciliation.md");
    for expected in [
        "P200-P262",
        "authoritative implementation range",
        "0001-0012",
        "P267",
        "0013",
        "P268",
        "0014",
        "never copied by version",
    ] {
        assert!(manifest.contains(expected), "manifest missing {expected}");
    }

    let ids = e2e_spec_ids(&repo_root().join("specs/e2e"));
    let unique: BTreeSet<u32> = ids.iter().copied().collect();
    assert_eq!(ids.len(), unique.len(), "P200+ e2e ids must be unique");
    assert_eq!(ids.iter().filter(|id| **id == 263).count(), 1);
    assert!(ids.contains(&264));
}

#[test]
fn p263_parity_map_adds_enterprise_requirements_without_completion_claim() {
    let map = read("docs/parity/agent-chat-capability-map.md");
    assert!(map.contains("P200-P262"));
    let rows = parity_rows(&map);
    let expected = [
        ("native_runtime_process", "missing"),
        ("native_runtime_session_restore", "missing"),
        ("durable_runtime_identity", "partial"),
        ("project_room_repo_binding", "partial"),
        ("worker_fleet_protocol", "partial"),
        ("durable_task_leases", "partial"),
        ("auth_rbac_quota", "partial"),
        ("artifact_audit_provenance", "partial"),
        ("operational_doctor_health", "missing"),
    ];
    for (id, status) in expected {
        let row = rows
            .get(id)
            .unwrap_or_else(|| panic!("missing parity row {id}"));
        assert_eq!(row[4], status, "unexpected status for {id}");
    }
    assert_eq!(rows["real_codex_execution"][4], "covered");
    assert_eq!(rows["matrix_bridge"][4], "partial");
    assert!(
        rows["project_room_repo_binding"][5].contains("P269"),
        "partial project authority status requires P269 evidence"
    );
    assert!(map.contains("cannot fully replace agent-chat yet"));
}

#[test]
fn p263_roadmap_orders_reconciled_enterprise_work() {
    let roadmap = read("docs/plans/2026-07-08-agent-chat-replacement-roadmap.md");
    for id in 263..=279 {
        assert!(roadmap.contains(&format!("P{id}")), "roadmap missing P{id}");
    }
    let ownership = roadmap.find("P264").expect("P264 ownership");
    let runtime_schema = roadmap.find("P267").expect("P267 schema");
    let artifact_schema = roadmap.find("P268").expect("P268 schema");
    assert!(ownership < runtime_schema && runtime_schema < artifact_schema);

    let immediate = roadmap
        .split("## Immediate Next Step")
        .nth(1)
        .expect("Immediate Next Step");
    assert!(immediate.contains("P264"));
    assert!(immediate.contains("ownership"));
    assert!(roadmap.contains("P267 uses migration `0013`"));
    assert!(roadmap.contains("P268 uses migration `0014`"));
}
