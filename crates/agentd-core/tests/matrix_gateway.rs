use agentd_core::ports::{
    MatrixCommandClass, MatrixCommandDisposition, MatrixExecutionSummaryStatus,
    MatrixGatewayDenialReason, MatrixGatewayMode,
};
use agentd_core::types::{MatrixCommandId, MatrixGatewayOutboxId};

#[test]
fn matrix_gateway_contract_uses_canonical_ids_and_closed_wire_values() {
    let command_id = MatrixCommandId::new();
    let outbox_id = MatrixGatewayOutboxId::new();
    assert!(command_id.as_str().starts_with("mc_"));
    assert!(outbox_id.as_str().starts_with("mo_"));

    for (class, value) in [
        (MatrixCommandClass::Execute, "execute"),
        (MatrixCommandClass::Status, "status"),
        (MatrixCommandClass::Cancel, "cancel"),
    ] {
        assert_eq!(class.as_str(), value);
    }
    assert_eq!(MatrixCommandDisposition::Accepted.as_str(), "accepted");
    assert_eq!(MatrixCommandDisposition::Replayed.as_str(), "replayed");
    assert_eq!(MatrixExecutionSummaryStatus::Failed.as_str(), "failed");
    assert_eq!(
        MatrixGatewayMode::ShadowReadOnly.as_str(),
        "shadow_read_only"
    );
    assert!(MatrixGatewayMode::Canary.permits_execution());
    assert!(MatrixGatewayMode::Active.permits_execution());
    assert!(!MatrixGatewayMode::Observe.permits_execution());
    assert_eq!(
        MatrixGatewayDenialReason::TransportIdentityMismatch.as_str(),
        "transport_identity_mismatch"
    );
}

#[test]
fn matrix_gateway_wire_values_round_trip_without_legacy_agent_identity() {
    let mode = serde_json::to_string(&MatrixGatewayMode::RolledBack).expect("serialize mode");
    let class = serde_json::to_string(&MatrixCommandClass::Execute).expect("serialize class");
    assert_eq!(mode, "\"rolled_back\"");
    assert_eq!(class, "\"execute\"");
    for forbidden in ["agent_name", "tmux", "pane", "worktree"] {
        assert!(!mode.contains(forbidden));
        assert!(!class.contains(forbidden));
    }
}
