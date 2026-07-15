//! Artifact tests for the P264 enterprise ownership boundary.

use std::collections::BTreeSet;
use std::path::PathBuf;

const OWNERSHIP_DOC: &str = "docs/specs/2026-07-10-enterprise-execution-ownership-boundary.md";
const PATH_B_DOC: &str = "docs/specs/2026-05-29-agentd-specify-boundary.md";
const ROADMAP_DOC: &str = "docs/plans/2026-07-08-agent-chat-replacement-roadmap.md";
const PARITY_MAP: &str = "docs/parity/agent-chat-capability-map.md";

const ROLES: [&str; 5] = [
    "SpecifyProjectAuthority",
    "AgentdControlPlane",
    "AgentdWorker",
    "OpenFabCertificationAuthority",
    "MatrixRobrixTransport",
];

const STATE_IDS: [&str; 18] = [
    "organization_team",
    "project_repository",
    "matrix_project_binding",
    "issue_requirement_spec",
    "product_workflow_state",
    "project_rbac_policy",
    "certification_policy_intent",
    "worker_registry",
    "agent_capability_registry",
    "execution_queue_lease",
    "runtime_session_record",
    "execution_run_checkpoint",
    "execution_artifact_index",
    "execution_audit_usage",
    "live_process_pty",
    "worktree_cache_transcript_spool",
    "certification_attestation",
    "matrix_identity_room_transport",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_doc(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

fn table_rows(document: &str) -> Vec<(&str, &str, &str)> {
    document
        .lines()
        .filter_map(|line| {
            let columns = line
                .split('|')
                .map(str::trim)
                .filter(|column| !column.is_empty())
                .collect::<Vec<_>>();
            if columns.len() != 4 || !columns[0].starts_with('`') {
                return None;
            }
            Some((
                columns[0].trim_matches('`'),
                columns[1].trim_matches('`'),
                line,
            ))
        })
        .collect()
}

fn owner_for<'a>(rows: &'a [(&str, &str, &str)], state_id: &str) -> &'a str {
    let matches = rows
        .iter()
        .filter(|(id, _, _)| *id == state_id)
        .collect::<Vec<_>>();
    assert_eq!(
        matches.len(),
        1,
        "state id {state_id} must occur exactly once"
    );
    matches[0].1
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
fn p264_ownership_contract_assigns_each_state_class_once() {
    let document = read_doc(OWNERSHIP_DOC);
    let rows = table_rows(&document);

    assert_eq!(
        rows.iter().map(|(id, _, _)| *id).collect::<BTreeSet<_>>(),
        STATE_IDS.into_iter().collect(),
        "ownership table must contain exactly the P264 state ids"
    );
    assert_eq!(rows.len(), STATE_IDS.len(), "state ids must not repeat");

    for (state_id, owner, raw) in rows {
        assert!(
            ROLES.contains(&owner),
            "state {state_id} has unknown owner {owner}"
        );
        assert!(
            !raw.contains("shared"),
            "shared ownership is forbidden: {raw}"
        );
        assert!(
            !raw.contains("agentd/Specify"),
            "compound ownership is forbidden: {raw}"
        );
    }
}

#[test]
fn p264_specify_owns_project_authority_not_execution_state() {
    let document = read_doc(OWNERSHIP_DOC);
    let rows = table_rows(&document);

    for state_id in [
        "organization_team",
        "project_repository",
        "matrix_project_binding",
        "issue_requirement_spec",
        "product_workflow_state",
        "project_rbac_policy",
        "certification_policy_intent",
    ] {
        assert_eq!(owner_for(&rows, state_id), "SpecifyProjectAuthority");
    }
    for state_id in [
        "worker_registry",
        "runtime_session_record",
        "execution_queue_lease",
        "execution_run_checkpoint",
        "worktree_cache_transcript_spool",
    ] {
        assert_ne!(owner_for(&rows, state_id), "SpecifyProjectAuthority");
    }
    assert!(document.contains(
        "`SpecifyProjectAuthority` MUST NOT own worker registrations, runtime sessions, execution leases, checkpoints, or transcripts."
    ));
}

#[test]
fn p264_agentd_control_plane_and_worker_have_disjoint_state() {
    let document = read_doc(OWNERSHIP_DOC);
    let rows = table_rows(&document);

    for state_id in [
        "worker_registry",
        "agent_capability_registry",
        "execution_queue_lease",
        "runtime_session_record",
        "execution_run_checkpoint",
        "execution_artifact_index",
        "execution_audit_usage",
    ] {
        assert_eq!(owner_for(&rows, state_id), "AgentdControlPlane");
    }
    for state_id in ["live_process_pty", "worktree_cache_transcript_spool"] {
        assert_eq!(owner_for(&rows, state_id), "AgentdWorker");
    }
    assert!(document.contains(
        "`AgentdWorker` MUST NOT become the durable source of truth for projects, specs, runs, tasks, leases, artifacts, or transcripts."
    ));
    assert!(document.contains("Fencing and lease recovery remain control-plane decisions"));
}

#[test]
fn p264_openfab_certifies_without_owning_delivery_or_execution() {
    let document = read_doc(OWNERSHIP_DOC);
    let rows = table_rows(&document);

    assert_eq!(
        owner_for(&rows, "certification_attestation"),
        "OpenFabCertificationAuthority"
    );
    for expected in [
        "`gate=none`",
        "`deliver` and `certify` are separate decisions",
        "certification results, signatures, and provenance attestations",
        "MUST NOT own execution queues, runtime sessions, task leases, commits, or pull requests",
    ] {
        assert!(
            document.contains(expected),
            "missing OpenFab rule: {expected}"
        );
    }
}

#[test]
fn p264_standalone_mode_uses_same_ports_and_explicit_precedence() {
    let document = read_doc(OWNERSHIP_DOC);

    for expected in [
        "`ProjectAuthorityPort`",
        "`WorkerFleetPort`",
        "`CertificationPort`",
        "`LocalProjectAuthority` implements `ProjectAuthorityPort`",
        "Specify is configured, `SpecifyProjectAuthority` is authoritative",
        "MUST fail closed and MUST NOT silently fall back to `LocalProjectAuthority`",
        "same stable project, repository, workflow, task, and policy identity model",
    ] {
        assert!(
            document.contains(expected),
            "missing deployment rule: {expected}"
        );
    }
}

#[test]
fn p264_existing_boundary_and_roadmap_reference_amendment() {
    let path_b = read_doc(PATH_B_DOC);
    let roadmap = read_doc(ROADMAP_DOC);
    let amendment_name = "2026-07-10-enterprise-execution-ownership-boundary.md";

    assert!(path_b.contains(amendment_name));
    assert!(path_b.contains("amended by P264"));
    assert!(roadmap.contains(amendment_name));
    assert!(roadmap.contains("Specify Project Authority"));
    assert!(roadmap.contains("Agentd Execution Control Plane"));

    let p264 = roadmap
        .find(amendment_name)
        .expect("roadmap references P264 amendment");
    let p267 = roadmap.find("P267").expect("roadmap has P267");
    assert!(p264 < p267, "ownership decision must precede P267");
}

#[test]
fn p264_parity_rows_name_resolved_owners() {
    let map = read_doc(PARITY_MAP);

    for (id, expected) in [
        ("project_room_repo_binding", "SpecifyProjectAuthority"),
        ("worker_fleet_protocol", "AgentdControlPlane"),
        ("durable_task_leases", "AgentdControlPlane"),
        ("operational_doctor_health", "AgentdControlPlane"),
    ] {
        assert!(
            parity_row(&map, id).contains(expected),
            "parity row {id} must name {expected}"
        );
    }

    let policy = parity_row(&map, "auth_rbac_quota");
    assert!(policy.contains("SpecifyProjectAuthority"));
    assert!(policy.contains("AgentdControlPlane"));

    let artifacts = parity_row(&map, "artifact_audit_provenance");
    assert!(artifacts.contains("AgentdControlPlane"));
    assert!(artifacts.contains("OpenFabCertificationAuthority"));

    assert!(!map.contains("agentd owns project/spec source of truth"));
}
