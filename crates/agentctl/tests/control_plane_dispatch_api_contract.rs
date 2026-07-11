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
fn p270_roadmap_and_parity_record_durable_lease_api_without_claiming_worker_protocol() {
    let roadmap = read("docs/plans/2026-07-08-agent-chat-replacement-roadmap.md");
    let parity = read("docs/parity/agent-chat-capability-map.md");
    let design = read("docs/specs/2026-07-10-control-plane-task-lease-api.md");
    let port = read("crates/agentd-core/src/ports/task_lease.rs");
    let migration = read("crates/agentd-store/migrations/0015_enterprise_task_leases.sql");
    let adapter = read("crates/agentd-store/src/task_lease_control_plane.rs");

    for expected in [
        "TaskLeasePort",
        "FencingToken",
        "BEGIN IMMEDIATE",
        "dispatch_queue.ticket",
        "P271",
        "P278",
    ] {
        assert!(design.contains(expected), "design missing {expected}");
    }
    for method in [
        "async fn dispatch",
        "async fn renew",
        "async fn release",
        "async fn cancel",
        "async fn validate_claim",
        "async fn expire_due",
    ] {
        assert!(port.contains(method), "port missing {method}");
    }
    for expected in [
        "CREATE TABLE execution_task_leases",
        "CREATE TABLE execution_task_lease_heads",
        "idx_execution_task_leases_one_active",
        "value = '15'",
    ] {
        assert!(migration.contains(expected), "migration missing {expected}");
    }
    assert!(adapter.contains("BEGIN IMMEDIATE"));
    assert!(adapter.contains("SqliteTaskLeaseControlPlane"));
    assert!(!adapter.contains("agent_scheduler_queue"));
    assert!(!adapter.contains("agent_scheduler_reservations"));

    for expected in ["P270", "TaskLeasePort", "0015_enterprise_task_leases.sql"] {
        assert!(roadmap.contains(expected), "roadmap missing {expected}");
        assert!(parity.contains(expected), "parity missing {expected}");
    }
    let leases = parity_row(&parity, "durable_task_leases");
    assert!(leases.contains("| partial |"));
    assert!(leases.contains("P271 adds required rejection audit"));
    for pending in [
        "worker protocol",
        "remaining report audit",
        "compatibility cutover",
    ] {
        assert!(leases.contains(pending), "lease row missing {pending}");
    }
    assert!(leases.contains("P270"));

    let immediate = roadmap
        .split("## Immediate Next Step")
        .nth(1)
        .expect("roadmap Immediate Next Step");
    assert!(immediate.contains("p271-control-plane-artifact-audit-api.spec.md"));
    for expected in [
        "ArtifactIndexPort",
        "ExecutionAuditPort",
        "UsageLedgerPort",
        "CertificationReferencePort",
    ] {
        assert!(
            immediate.contains(expected),
            "P271 summary missing {expected}"
        );
    }
    assert!(immediate.contains("p272-runtime-compatibility-port.spec.md"));
}
