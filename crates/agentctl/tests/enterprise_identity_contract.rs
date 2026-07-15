//! Artifact tests for the P265 enterprise runtime/worker identity contract.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const IDENTITY_DOC: &str = "docs/specs/2026-07-10-enterprise-runtime-worker-identity-contract.md";
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

fn comma_set(value: &str) -> BTreeSet<&str> {
    value
        .split(',')
        .map(str::trim)
        .map(unquote)
        .filter(|value| !value.is_empty() && *value != "none")
        .collect()
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
fn p265_identity_catalog_uses_distinct_opaque_ids() {
    let document = read_doc(IDENTITY_DOC);
    let section = section_between(
        &document,
        "## 2. Canonical Identity Catalog",
        "## 3. Legacy Value Classification",
    );
    let rows = table_rows(section, 5);
    let actual = rows
        .iter()
        .map(|row| {
            (
                unquote(row[0]),
                (unquote(row[1]), unquote(row[2]), unquote(row[3])),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let expected = BTreeMap::from([
        ("AgentProfileId", ("ap_", "AgentdControlPlane", "durable")),
        ("ExecutionRunId", ("r_", "AgentdControlPlane", "durable")),
        ("ExecutionTaskId", ("tr_", "AgentdControlPlane", "durable")),
        ("LeaseId", ("ls_", "AgentdControlPlane", "durable")),
        ("RuntimeAttemptId", ("ra_", "AgentdControlPlane", "durable")),
        ("RuntimeSessionId", ("rs_", "AgentdControlPlane", "durable")),
        ("WorkerId", ("wk_", "AgentdControlPlane", "durable")),
        (
            "WorkerIncarnationId",
            ("wi_", "AgentdControlPlane", "durable"),
        ),
    ]);

    assert_eq!(actual, expected);
    assert_eq!(
        actual
            .values()
            .map(|(prefix, _, _)| *prefix)
            .collect::<BTreeSet<_>>()
            .len(),
        actual.len(),
        "canonical prefixes must be distinct"
    );
    for rule in [
        "ULID payload",
        "IDs are immutable and never reused",
        "Consumers MUST treat every canonical id as opaque",
        "`r_` and `tr_` preserve the existing core prefixes",
    ] {
        assert!(document.contains(rule), "missing canonical id rule: {rule}");
    }
}

#[test]
fn p265_legacy_runtime_fields_are_never_canonical_ids() {
    let document = read_doc(IDENTITY_DOC);
    let section = section_between(
        &document,
        "## 3. Legacy Value Classification",
        "## 4. Relationship Model",
    );
    let rows = table_rows(section, 3);
    let actual = rows
        .iter()
        .map(|row| (unquote(row[0]), unquote(row[1])))
        .collect::<BTreeMap<_, _>>();
    let expected = BTreeMap::from([
        ("agents.id", "import alias"),
        ("agents.mxid", "transport metadata"),
        ("agents.server", "placement metadata"),
        ("backend_target", "attempt locator"),
        ("dispatch_queue.ticket", "import input"),
        ("host_name", "placement metadata"),
        ("native_session_ref", "resume locator"),
        ("pane_id", "attempt locator"),
        ("pid", "attempt metadata"),
        ("session_name", "attempt locator"),
        ("workdir", "attempt metadata"),
    ]);

    assert_eq!(actual, expected);
    assert!(
        actual
            .values()
            .all(|classification| !classification.contains("canonical"))
    );
    assert!(document.contains(
        "Matrix room ids, repository paths, worktree paths, and provider resume refs MUST NOT be parsed into agentd canonical ids."
    ));
}

#[test]
fn p265_worker_incarnation_rejects_stale_reports() {
    let document = read_doc(IDENTITY_DOC);

    for rule in [
        "`WorkerId` remains unchanged across daemon restarts and registrations",
        "Every accepted registration allocates a new `WorkerIncarnationId`",
        "The new incarnation atomically supersedes the prior incarnation",
        "heartbeat, capacity, live-process, artifact, and lease reports from a superseded incarnation MUST be rejected and audited",
        "Superseding an incarnation does not delete the worker enrollment or its history",
    ] {
        assert!(document.contains(rule), "missing worker rule: {rule}");
    }
}

#[test]
fn p265_runtime_session_survives_attempt_loss_or_becomes_explicitly_lost() {
    let document = read_doc(IDENTITY_DOC);

    for rule in [
        "A `RuntimeSessionId` is stable across spawn and resume attempts",
        "Every spawn or resume allocates a new `RuntimeAttemptId`",
        "moves the runtime session to `resume_pending`",
        "keeps the same `RuntimeSessionId` and creates a new `RuntimeAttemptId`",
        "becomes terminal `lost` with reason `runtime_gone`",
        "A PID, PTY, backend address, session name, and native resume ref never replace either runtime id",
    ] {
        assert!(document.contains(rule), "missing runtime rule: {rule}");
    }
}

#[test]
fn p265_lease_fencing_rejects_stale_mutations() {
    let document = read_doc(IDENTITY_DOC);

    for rule in [
        "one `ExecutionTaskId`, one `WorkerIncarnationId`, one `LeaseId`, and one `FencingToken`",
        "strictly greater than every earlier token for that task",
        "Reports carrying an older token MUST be rejected and audited",
        "Reports for a terminal lease MUST be rejected and audited",
        "Retry allocates a new `LeaseId`",
        "Wall-clock timestamps never override lease-token ordering",
    ] {
        assert!(document.contains(rule), "missing fencing rule: {rule}");
    }
}

#[test]
fn p265_execution_relationships_are_unambiguous() {
    let document = read_doc(IDENTITY_DOC);
    let section = section_between(
        &document,
        "## 4. Relationship Model",
        "## 5. Lifecycle Contracts",
    );
    let rows = table_rows(section, 4);
    let parents = rows
        .iter()
        .map(|row| (unquote(row[0]), unquote(row[1])))
        .collect::<BTreeMap<_, _>>();
    let expected = BTreeMap::from([
        ("AgentProfileId", "AgentdControlPlane catalog root"),
        ("ExecutionRunId", "ProjectAuthorityRef"),
        ("ExecutionTaskId", "ExecutionRunId"),
        ("LeaseId", "ExecutionTaskId"),
        ("RuntimeAttemptId", "RuntimeSessionId"),
        ("RuntimeSessionId", "ExecutionTaskId"),
        ("WorkerId", "AgentdControlPlane enrollment root"),
        ("WorkerIncarnationId", "WorkerId"),
    ]);

    assert_eq!(parents, expected);
    assert!(document.contains(
        "Live process metadata is scoped by both `RuntimeAttemptId` and `WorkerIncarnationId`."
    ));
    assert!(document.contains(
        "Relationships use references between ids; concatenated or compound ids are forbidden."
    ));
}

#[test]
fn p265_lifecycle_tables_define_terminal_and_recovery_states() {
    let document = read_doc(IDENTITY_DOC);
    let section = section_between(
        &document,
        "## 5. Lifecycle Contracts",
        "## 6. Failure and Recovery Invariants",
    );
    let rows = table_rows(section, 4);
    let actual = rows
        .iter()
        .map(|row| (unquote(row[0]), (comma_set(row[1]), comma_set(row[2]))))
        .collect::<BTreeMap<_, _>>();
    let expected = BTreeMap::from([
        (
            "AgentProfile",
            (
                BTreeSet::from(["active", "disabled", "retired"]),
                BTreeSet::from(["retired"]),
            ),
        ),
        (
            "ExecutionRun",
            (
                BTreeSet::from(["cancelled", "failed", "pending", "running", "succeeded"]),
                BTreeSet::from(["cancelled", "failed", "succeeded"]),
            ),
        ),
        (
            "ExecutionTask",
            (
                BTreeSet::from([
                    "cancelled",
                    "dead_letter",
                    "failed",
                    "leased",
                    "queued",
                    "running",
                    "succeeded",
                ]),
                BTreeSet::from(["cancelled", "dead_letter", "failed", "succeeded"]),
            ),
        ),
        (
            "Lease",
            (
                BTreeSet::from(["active", "cancelled", "expired", "released", "superseded"]),
                BTreeSet::from(["cancelled", "expired", "released", "superseded"]),
            ),
        ),
        (
            "RuntimeAttempt",
            (
                BTreeSet::from(["exited", "gone", "running", "starting"]),
                BTreeSet::from(["exited", "gone"]),
            ),
        ),
        (
            "RuntimeSession",
            (
                BTreeSet::from([
                    "cancelled",
                    "completed",
                    "failed",
                    "lost",
                    "requested",
                    "resume_pending",
                    "running",
                    "starting",
                ]),
                BTreeSet::from(["cancelled", "completed", "failed", "lost"]),
            ),
        ),
        (
            "Worker",
            (
                BTreeSet::from(["draining", "offline", "online", "retired"]),
                BTreeSet::from(["retired"]),
            ),
        ),
    ]);

    assert_eq!(actual, expected);
    assert!(document.contains("Terminal records MUST NOT transition back to an active state."));
    assert!(document.contains(
        "Re-registration, resume, retry, or replacement allocates the new incarnation, attempt, lease, or task identity named in the table."
    ));
}

#[test]
fn p265_roadmap_and_parity_advance_after_identity_contract() {
    let roadmap = read_doc(ROADMAP_DOC);
    let parity = read_doc(PARITY_MAP);
    let contract = "2026-07-10-enterprise-runtime-worker-identity-contract.md";

    assert!(roadmap.contains(contract));
    assert!(parity.contains(contract));

    let identity_row = parity_row(&parity, "durable_runtime_identity");
    assert!(identity_row.contains("| partial |"));
    assert!(identity_row.contains("P265"));
    assert!(identity_row.contains("implementation remains pending"));

    let immediate = roadmap
        .split("## Immediate Next Step")
        .nth(1)
        .expect("roadmap Immediate Next Step");
    assert!(immediate.contains("P266"));
    assert!(immediate.contains("foreign authority reference model"));
    assert!(immediate.contains("MUST NOT add agentd-owned project tables"));
}
