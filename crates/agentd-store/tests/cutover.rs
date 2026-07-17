use agentd_core::ports::{
    CursorHandoff, CutoverLedgerPort, CutoverPlan, CutoverSourceManifest, CutoverState,
    CutoverSurface, CutoverTransition, LegacyIdMapping, ShadowDecision,
};
use agentd_core::types::{CutoverId, CutoverSourceId};
use agentd_store::{SqliteCutoverLedger, SqliteStore};

fn digest(byte: char) -> String {
    std::iter::repeat_n(byte, 64).collect()
}

#[tokio::test]
async fn cutover_ledger_is_exactly_idempotent_and_immutable() {
    let directory = tempfile::tempdir().expect("temporary cutover database");
    let store = SqliteStore::connect(directory.path().join("agentd.db"))
        .await
        .expect("store");
    let ledger = SqliteCutoverLedger::new(store.pool().clone());
    let cutover_id = CutoverId::new();
    let plan = CutoverPlan {
        id: cutover_id.clone(),
        source_root_sha256: digest('a'),
        target_database_sha256: None,
        rollback_window_expires_at: 10_000,
        created_at: 1,
    };
    assert_eq!(
        ledger.create_cutover(&plan).await.expect("create").state,
        CutoverState::Planned
    );
    assert_eq!(
        ledger
            .create_cutover(&plan)
            .await
            .expect("exact replay")
            .plan,
        plan
    );

    let transition = CutoverTransition {
        cutover_id: cutover_id.clone(),
        expected_state: CutoverState::Planned,
        next_state: CutoverState::Importing,
        idempotency_key: "begin-import".to_string(),
        input_sha256: digest('b'),
        authority_owner: "agent_chat_read_only".to_string(),
        occurred_at: 2,
    };
    assert_eq!(
        ledger
            .transition_cutover(&transition)
            .await
            .expect("transition")
            .state,
        CutoverState::Importing
    );
    assert_eq!(
        ledger
            .transition_cutover(&transition)
            .await
            .expect("transition replay")
            .state,
        CutoverState::Importing
    );
    assert!(
        ledger
            .transition_cutover(&CutoverTransition {
                input_sha256: digest('c'),
                ..transition.clone()
            })
            .await
            .is_err()
    );

    let source = CutoverSourceManifest {
        id: CutoverSourceId::new(),
        cutover_id: cutover_id.clone(),
        source_sha256: plan.source_root_sha256.clone(),
        file_count: 6,
        record_count: 12,
        captured_at: 3,
    };
    assert_eq!(ledger.record_source(&source).await.expect("source"), source);
    let mapping = LegacyIdMapping {
        cutover_id: cutover_id.clone(),
        surface: CutoverSurface::Task,
        legacy_id_sha256: digest('d'),
        native_id: "task-native-1".to_string(),
        native_record_sha256: digest('e'),
        mapped_at: 4,
    };
    assert_eq!(
        ledger.record_mapping(&mapping).await.expect("mapping"),
        mapping
    );
    let shadow = ShadowDecision {
        cutover_id: cutover_id.clone(),
        surface: CutoverSurface::Task,
        decision_key_sha256: digest('f'),
        legacy_decision_sha256: digest('1'),
        native_decision_sha256: digest('1'),
        matched: true,
        reason_code: "matched".to_string(),
        observed_at: 5,
    };
    assert_eq!(ledger.record_shadow(&shadow).await.expect("shadow"), shadow);
    let handoff = CursorHandoff {
        cutover_id: cutover_id.clone(),
        project_ref_sha256: digest('2'),
        previous_cursor_sha256: digest('3'),
        next_cursor: "matrix-sync-100".to_string(),
        authority_owner: "agentd".to_string(),
        acknowledged: true,
        handed_off_at: 6,
    };
    ledger
        .record_cursor_handoff(&handoff)
        .await
        .expect("handoff");

    assert_eq!(
        ledger.mappings(&cutover_id).await.expect("mappings"),
        [mapping]
    );
    assert_eq!(
        ledger.shadows(&cutover_id).await.expect("shadows"),
        [shadow]
    );
    assert_eq!(
        ledger.cursor_handoffs(&cutover_id).await.expect("handoffs"),
        [handoff]
    );
    assert!(
        sqlx::query("UPDATE cutover_sources SET source_sha256 = ? WHERE id = ?")
            .bind(digest('4'))
            .bind(source.id.as_str())
            .execute(store.pool())
            .await
            .is_err(),
        "source evidence must be immutable"
    );
}

#[tokio::test]
async fn cutover_ledger_rejects_skipped_state_edges() {
    let directory = tempfile::tempdir().expect("temporary cutover database");
    let store = SqliteStore::connect(directory.path().join("agentd.db"))
        .await
        .expect("store");
    let ledger = SqliteCutoverLedger::new(store.pool().clone());
    let plan = CutoverPlan {
        id: CutoverId::new(),
        source_root_sha256: digest('a'),
        target_database_sha256: None,
        rollback_window_expires_at: 10_000,
        created_at: 1,
    };
    ledger.create_cutover(&plan).await.expect("create");
    let error = ledger
        .transition_cutover(&CutoverTransition {
            cutover_id: plan.id,
            expected_state: CutoverState::Planned,
            next_state: CutoverState::Active,
            idempotency_key: "skip".to_string(),
            input_sha256: digest('b'),
            authority_owner: "agentd".to_string(),
            occurred_at: 2,
        })
        .await
        .expect_err("skipped states must fail");
    assert!(error.to_string().contains("illegal cutover transition"));
}
