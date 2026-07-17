//! Repository ownership proof for the AD-E2 durable scheduler code candidate.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(relative: &str) -> String {
    let path = root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

#[test]
fn ad_e2_candidate_assigns_scheduler_capabilities_to_canonical_code() {
    let owned = [
        (
            "crates/agentd-core/src/ports/fleet_scheduler.rs",
            &[
                "FleetSchedulerPort",
                "WorkerAvailability",
                "FleetAssignment",
                "ArtifactUploadAckRequest",
                "FleetSideEffectRequest",
                "FleetReapSummary",
                "FleetExplain",
            ][..],
        ),
        (
            "crates/agentd-store/src/fleet_scheduler.rs",
            &[
                "SqliteFleetScheduler",
                "SqliteImmediateTransaction",
                "dispatch_in_transaction",
                "check_epoch",
                "record_fencing_rejection",
                "task.dead_letter",
            ][..],
        ),
        (
            "crates/agentd-store/src/util.rs",
            &[
                "SqliteImmediateTransaction",
                "BEGIN IMMEDIATE",
                "close_on_drop",
            ][..],
        ),
        (
            "crates/agentd-bin/src/fleet.rs",
            &[
                "EnterpriseFleetService",
                "WorkloadIdentityPort",
                "trusted_clock",
                "acknowledge_artifact_upload",
                "admit_side_effect",
            ][..],
        ),
        (
            "crates/agentd-store/src/execution_evidence_control_plane.rs",
            &[
                "authorize_artifact_upload",
                "enterprise_artifact_upload_acknowledgements",
            ][..],
        ),
    ];
    for (path, symbols) in owned {
        let source = read(path);
        for symbol in symbols {
            assert!(source.contains(symbol), "{path} does not own {symbol}");
        }
    }
}

#[test]
fn ad_e2_schema_is_additive_structured_and_separate_from_compatibility_scheduler() {
    let migration = read("crates/agentd-store/migrations/0018_enterprise_fleet_scheduler.sql");
    for table in [
        "enterprise_fleet_queue",
        "enterprise_worker_availability",
        "enterprise_scheduler_outbox",
        "enterprise_scheduler_report_receipts",
        "enterprise_artifact_upload_acknowledgements",
        "enterprise_side_effect_admissions",
        "enterprise_fencing_rejections",
    ] {
        assert!(migration.contains(table), "missing {table}");
    }
    assert!(!migration.contains("REFERENCES agent_scheduler_queue"));
    for forbidden in [
        "secret_bytes",
        "raw_error",
        "transcript_json",
        "workdir",
        "matrix_room_id",
        "tmux_target",
    ] {
        assert!(
            !migration.to_ascii_lowercase().contains(forbidden),
            "enterprise scheduler schema owns forbidden field {forbidden}"
        );
    }
}

#[test]
fn ad_e2_evidence_covers_atomic_fencing_retry_reap_and_fail_closed_service() {
    let store = read("crates/agentd-store/tests/enterprise_fleet_scheduler.rs");
    let service = read("crates/agentd-bin/tests/enterprise_fleet.rs");
    let roadmap = read("docs/plans/2026-07-09-agentd-native-runtime-roadmap.md");
    let checklist = read("docs/acceptance/ad-e-roadmap-manual-checklist.md");

    for evidence in [
        "queue_pull_lease_outbox_reports_and_artifact_ack_are_fenced_and_idempotent",
        "retry_reassignment_reaper_and_dead_letter_never_reuse_fencing_tokens",
        "FleetQueueStatus::DeadLetter",
    ] {
        assert!(
            store.contains(evidence),
            "missing store evidence {evidence}"
        );
    }
    for evidence in [
        "fleet_service_uses_authenticated_workload_and_trusted_time",
        "rejects_missing_or_denied_identity_before_scheduler_mutation",
    ] {
        assert!(
            service.contains(evidence),
            "missing service evidence {evidence}"
        );
    }
    assert!(roadmap.contains("AD-E2 code-complete candidate"));
    assert!(roadmap.contains("not an AD-E2 or FSF-3 exit"));
    assert!(!roadmap.contains("AD-E2: PASS"));
    assert!(!roadmap.contains("FSF-3: PASS"));
    for scenario in [
        "control plane during queued",
        "Kill and replace workers",
        "stale fencing tokens",
        "partial-upload",
    ] {
        assert!(
            checklist.contains(scenario),
            "manual checklist missing {scenario}"
        );
    }
}
