//! Repository ownership proof for the AD-E3 Matrix/Robrix code candidate.

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
fn ad_e3_candidate_assigns_gateway_cutover_and_robrix_capabilities_to_canonical_code() {
    let owned = [
        (
            "crates/agentd-core/src/ports/matrix_gateway.rs",
            &[
                "MatrixGatewayPort",
                "MatrixGatewayIdentityPort",
                "MatrixGatewayDeliveryPort",
                "MatrixTransportProvenance",
                "MatrixExecutionSummaryStatus",
                "RobrixRunView",
                "RobrixArtifactView",
                "RobrixApprovalView",
                "RobrixEvidenceView",
                "MatrixGatewayRollbackManifest",
            ][..],
        ),
        (
            "crates/agentd-store/src/matrix_gateway.rs",
            &[
                "SqliteMatrixGateway",
                "BEGIN IMMEDIATE",
                "load_binding_connection",
                "existing_receipt_connection",
                "previous_sync_cursor",
                "semantic_summary",
                "load_robrix_run",
                "mark_outbox_delivered",
                "record_state_mapping",
            ][..],
        ),
        (
            "crates/agentd-bin/src/matrix_gateway.rs",
            &[
                "AgentdMatrixGateway",
                "authenticate_matrix_source",
                "normalize_command",
                "deliver_pending",
                "trusted_clock",
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
fn ad_e3_schema_is_immutable_structured_and_excludes_transcript_payloads() {
    let migration = read("crates/agentd-store/migrations/0019_matrix_gateway_cutover.sql");
    for table in [
        "matrix_gateway_project_bindings",
        "matrix_gateway_commands",
        "matrix_gateway_inbox",
        "matrix_gateway_outbox",
        "matrix_gateway_cutover_history",
        "matrix_gateway_state_mappings",
    ] {
        assert!(migration.contains(table), "missing {table}");
    }
    for trigger in [
        "trg_matrix_gateway_commands_no_update",
        "trg_matrix_gateway_inbox_no_update",
        "trg_matrix_gateway_cutover_no_update",
        "trg_matrix_gateway_state_mappings_no_update",
    ] {
        assert!(migration.contains(trigger), "missing {trigger}");
    }
    for forbidden in [
        "raw_body",
        "transcript_json",
        "attachment_bytes",
        "worktree_path",
        "tmux_target",
        "agent_name",
    ] {
        assert!(
            !migration.to_ascii_lowercase().contains(forbidden),
            "native Matrix schema owns forbidden field {forbidden}"
        );
    }
}

#[test]
fn ad_e3_evidence_covers_atomic_replay_shadow_rollback_delivery_and_closed_startup() {
    let store = read("crates/agentd-store/tests/matrix_gateway.rs");
    let service = read("crates/agentd-bin/tests/matrix_gateway.rs");
    let roadmap = read("docs/plans/2026-07-09-agentd-native-runtime-roadmap.md");
    let checklist = read("docs/acceptance/ad-e-roadmap-manual-checklist.md");

    for evidence in [
        "observe_shadow_canary_handoff_and_replay_are_atomic_and_idempotent",
        "cursor_conflict_denial_and_rollback_never_create_execution_side_effects",
        "native_gateway_schema_stores_content_references_not_raw_matrix_transcripts",
        "MatrixCommandDisposition::Replayed",
        "mark_outbox_delivered",
    ] {
        assert!(
            store.contains(evidence),
            "missing store evidence {evidence}"
        );
    }
    for evidence in [
        "matrix_gateway_uses_authenticated_provenance_and_trusted_time",
        "forged_sender_is_rejected_before_gateway_mutation",
        "pending_semantic_summaries_are_delivered_and_acknowledged_in_order",
        "missing_enterprise_provider_fails_gateway_startup_closed",
    ] {
        assert!(
            service.contains(evidence),
            "missing service evidence {evidence}"
        );
    }
    assert!(roadmap.contains("AD-E3 code-complete candidate"));
    assert!(roadmap.contains("not an AD-E3 or FSF-4 exit"));
    assert!(!roadmap.contains("AD-E3: PASS"));
    assert!(!roadmap.contains("FSF-4: PASS"));
    for scenario in [
        "Race command ingress",
        "before delivery acknowledgement",
        "Robrix project/run/task/artifact/approval/evidence",
    ] {
        assert!(
            checklist.contains(scenario),
            "manual checklist missing {scenario}"
        );
    }
}
