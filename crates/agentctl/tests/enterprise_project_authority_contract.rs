//! Artifact tests for the P266 project authority reference contract.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const AUTHORITY_DOC: &str =
    "docs/specs/2026-07-10-enterprise-project-room-repo-reference-contract.md";
const ROADMAP_DOC: &str = "docs/plans/2026-07-08-agent-chat-replacement-roadmap.md";
const PARITY_MAP: &str = "docs/parity/agent-chat-capability-map.md";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_doc(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

fn section_between<'a>(document: &'a str, start: &str, end: &str) -> &'a str {
    let start_index = document
        .find(start)
        .unwrap_or_else(|| panic!("missing section {start}"));
    let content = &document[start_index + start.len()..];
    let end_index = content
        .find(end)
        .unwrap_or_else(|| panic!("missing section boundary {end}"));
    &content[..end_index]
}

fn table_rows(section: &str, columns: usize) -> Vec<Vec<&str>> {
    section
        .lines()
        .filter_map(|line| {
            let values = line
                .split('|')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            (values.len() == columns && values[0].starts_with('`')).then_some(values)
        })
        .collect()
}

fn unquote(value: &str) -> &str {
    value.trim_matches('`')
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
fn p266_resource_catalog_assigns_owner_kind_and_versioning() {
    let document = read_doc(AUTHORITY_DOC);
    let section = section_between(
        &document,
        "## 2. Resource Reference Catalog",
        "## 3. Project Execution Snapshot",
    );
    let rows = table_rows(section, 4);
    let actual = rows
        .iter()
        .map(|row| {
            (
                unquote(row[0]),
                (unquote(row[1]), unquote(row[2]), unquote(row[3])),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let project_owner = "SpecifyProjectAuthority";
    let versioned = "immutable version";
    let expected = BTreeMap::from([
        (
            "CertificationPolicyVersionRef",
            (project_owner, "certification_policy", versioned),
        ),
        (
            "FrozenSpecVersionRef",
            (project_owner, "frozen_spec", versioned),
        ),
        ("IssueRef", (project_owner, "issue", versioned)),
        (
            "MatrixRoomRef",
            (
                "MatrixRobrixTransport",
                "matrix_room",
                "transport state version",
            ),
        ),
        (
            "OrganizationRef",
            (project_owner, "organization", versioned),
        ),
        (
            "ProductWorkflowRef",
            (project_owner, "product_workflow", versioned),
        ),
        (
            "ProjectExecutionSnapshotRef",
            (project_owner, "execution_snapshot", versioned),
        ),
        ("ProjectRef", (project_owner, "project", versioned)),
        (
            "ProjectRoomBindingRef",
            (project_owner, "project_room_binding", versioned),
        ),
        (
            "QuotaPolicyVersionRef",
            (project_owner, "quota_policy", versioned),
        ),
        (
            "RbacPolicyVersionRef",
            (project_owner, "rbac_policy", versioned),
        ),
        ("RepositoryRef", (project_owner, "repository", versioned)),
        ("RequirementRef", (project_owner, "requirement", versioned)),
        ("TeamRef", (project_owner, "team", versioned)),
    ]);

    assert_eq!(actual, expected);
    assert!(
        actual
            .values()
            .all(|(owner, _, _)| *owner != "AgentdControlPlane")
    );
    assert!(document.contains(
        "Authority-owned reference equality compares `AuthorityKey`, `ResourceKind`, `ResourceId`, and `ResourceVersion`."
    ));
}

#[test]
fn p266_execution_snapshot_is_immutable_complete_and_run_pinned() {
    let document = read_doc(AUTHORITY_DOC);
    let section = section_between(
        &document,
        "## 3. Project Execution Snapshot",
        "## 4. Repository Binding Rules",
    );
    let rows = table_rows(section, 3);
    let fields = rows
        .iter()
        .map(|row| (unquote(row[0]), unquote(row[1])))
        .collect::<BTreeMap<_, _>>();
    let required = BTreeSet::from([
        "authority_key",
        "authority_revision",
        "certification_policy_version_ref",
        "content_sha256",
        "frozen_spec_version_ref",
        "issue_ref",
        "issued_at",
        "offline_recovery_policy",
        "organization_ref",
        "product_workflow_ref",
        "project_ref",
        "quota_policy_version_ref",
        "rbac_policy_version_ref",
        "repository_bindings",
        "requirement_refs",
        "room_bindings",
        "snapshot_ref",
        "team_refs",
        "valid_until",
    ]);

    assert_eq!(fields.keys().copied().collect::<BTreeSet<_>>(), required);
    assert!(
        fields
            .values()
            .all(|requirement| *requirement == "required")
    );
    for rule in [
        "Each `ExecutionRunId` pins exactly one `ProjectExecutionSnapshotRef`",
        "MUST NOT resolve `latest` after the run is created",
        "Snapshot content is immutable",
    ] {
        assert!(document.contains(rule), "missing snapshot rule: {rule}");
    }
}

#[test]
fn p266_repository_binding_requires_one_target_and_base_commit() {
    let document = read_doc(AUTHORITY_DOC);

    for rule in [
        "one or more versioned repository bindings",
        "exactly one target `RepositoryRef`",
        "exactly one immutable base commit SHA",
        "Additional repository bindings are read-only execution inputs",
        "Multi-repository writes require separate coordinated execution runs",
        "Remote URL, forge slug, branch name, checkout path, worktree path, and local filesystem path are locators or metadata, never `RepositoryRef` identity",
    ] {
        assert!(document.contains(rule), "missing repository rule: {rule}");
    }
}

#[test]
fn p266_room_binding_has_single_command_owner_and_pinned_rbac() {
    let document = read_doc(AUTHORITY_DOC);

    for rule in [
        "A project may have zero or more active room bindings",
        "at most one active `command` binding per `AuthorityKey`",
        "`ProjectRoomBindingRef` and `ProjectExecutionSnapshotRef`",
        "Room membership is transport input, not sufficient authorization",
        "pinned `RbacPolicyVersionRef`",
        "A bare Matrix room id MUST NOT dispatch enterprise work",
    ] {
        assert!(document.contains(rule), "missing room rule: {rule}");
    }
}

#[test]
fn p266_authority_validation_and_offline_recovery_fail_closed() {
    let document = read_doc(AUTHORITY_DOC);
    let section = section_between(
        &document,
        "## 6. Authority Validation and Recovery",
        "## 7. Authority Rebind",
    );
    let rows = table_rows(section, 5);
    let decisions = rows
        .iter()
        .map(|row| {
            (
                (unquote(row[0]), unquote(row[1]), unquote(row[2])),
                unquote(row[3]),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let expected = BTreeMap::from([
        (
            ("existing_recovery", "authority_live", "validated_pinned"),
            "allow",
        ),
        (
            (
                "existing_recovery",
                "authority_unavailable",
                "allow_pinned_unexpired_unchanged",
            ),
            "allow",
        ),
        (
            (
                "existing_recovery",
                "authority_unavailable",
                "deny_or_missing",
            ),
            "deny",
        ),
        (
            (
                "existing_recovery",
                "authority_unavailable",
                "expired_or_changed",
            ),
            "deny",
        ),
        (
            (
                "new_execution",
                "configured_specify_live",
                "validated_current",
            ),
            "allow",
        ),
        (
            ("new_execution", "configured_specify_unavailable", "any"),
            "deny",
        ),
        (
            ("new_execution", "local_explicit_live", "validated_current"),
            "allow",
        ),
    ]);

    assert_eq!(decisions, expected);
    assert!(document.contains("The default offline recovery policy is `deny`."));
    assert!(
        document.contains("Configured Specify failure MUST NOT select `LocalProjectAuthority`.")
    );
}

#[test]
fn p266_authority_rebind_is_explicit_versioned_and_non_rewriting() {
    let document = read_doc(AUTHORITY_DOC);

    for rule in [
        "`AuthorityRebindRecord` is immutable",
        "old authority key and resource refs",
        "new authority key and resource refs",
        "operator, reason, created time, and mapping hash",
        "Reference equality includes `AuthorityKey` even when two resource id strings match",
        "Historical runs, artifacts, audit events, and certification requests retain their original snapshot and authority references",
        "No background job rewrites historical references in place",
    ] {
        assert!(document.contains(rule), "missing rebind rule: {rule}");
    }
}

#[test]
fn p266_legacy_project_fields_are_classified_non_authoritative() {
    let document = read_doc(AUTHORITY_DOC);
    let section = section_between(
        &document,
        "## 8. Base Compatibility Classification",
        "## 9. Consequences",
    );
    let rows = table_rows(section, 3);
    let actual = rows
        .iter()
        .map(|row| (unquote(row[0]), unquote(row[1])))
        .collect::<BTreeMap<_, _>>();
    let expected = BTreeMap::from([
        ("agent_scheduler_queue.room", "transport hint"),
        ("issues.id", "cache"),
        ("issues.project_id", "import alias"),
        ("matrix_events.project_id", "projection"),
        ("matrix_events.room_id", "transport hint"),
        ("projects.github_repo", "locator"),
        ("projects.id", "import alias"),
        ("projects.matrix_room_id", "transport hint"),
        ("projects.name", "projection"),
        ("projects.repo_path", "locator"),
        ("runs.project_id", "import alias"),
    ]);

    assert_eq!(actual, expected);
    assert!(document.contains(
        "None of these base fields is a project authority record, canonical `RepositoryRef`, or canonical `ProjectRoomBindingRef`."
    ));
}

#[test]
fn p266_roadmap_and_parity_advance_without_project_implementation() {
    let roadmap = read_doc(ROADMAP_DOC);
    let parity = read_doc(PARITY_MAP);
    let contract = "2026-07-10-enterprise-project-room-repo-reference-contract.md";

    assert!(roadmap.contains(contract));
    assert!(parity.contains(contract));

    let row = parity_row(&parity, "project_room_repo_binding");
    assert!(row.contains("| partial |"));
    assert!(row.contains("P266"));
    assert!(row.contains("P269"));
    assert!(row.contains("Specify network and durable pinning integration remain pending"));

    let immediate = roadmap
        .split("## Immediate Next Step")
        .nth(1)
        .expect("roadmap Immediate Next Step");
    assert!(immediate.contains("P267"));
    assert!(immediate.contains("first enterprise agent/worker schema slice"));
}
