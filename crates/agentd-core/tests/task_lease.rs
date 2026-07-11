use std::sync::Mutex;

use agentd_core::ports::{
    TaskLeaseCloseRequest, TaskLeaseDispatchRequest, TaskLeaseError, TaskLeasePort,
    TaskLeaseRenewRequest,
};
use agentd_core::types::{
    FencingToken, LeaseId, LeaseStatus, TaskLeaseClaim, TaskLeaseGrant, TaskRunId,
    WorkerIncarnationId,
};

#[derive(Default)]
struct RecordingTaskLeasePort {
    calls: Mutex<Vec<String>>,
}

impl RecordingTaskLeasePort {
    fn record(&self, value: String) {
        self.calls.lock().expect("calls lock").push(value);
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("calls lock").clone()
    }
}

fn grant() -> TaskLeaseGrant {
    TaskLeaseGrant {
        lease_id: LeaseId::from_string("ls_01ARZ3NDEKTSV4RRFFQ69G5FAV"),
        execution_task_id: TaskRunId::from_string("tr_01ARZ3NDEKTSV4RRFFQ69G5FAW"),
        worker_incarnation_id: WorkerIncarnationId::from_string("wi_01ARZ3NDEKTSV4RRFFQ69G5FAX"),
        fencing_token: FencingToken::new(7).expect("positive token"),
        status: LeaseStatus::Active,
        acquired_at: 100,
        expires_at: 200,
        renewed_at: None,
        terminal_at: None,
        terminal_reason: None,
        record_version: 1,
    }
}

#[async_trait::async_trait]
impl TaskLeasePort for RecordingTaskLeasePort {
    async fn dispatch(
        &self,
        request: &TaskLeaseDispatchRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        self.record(format!(
            "dispatch:{}:{}:{}:{}",
            request.execution_task_id,
            request.worker_incarnation_id,
            request.observed_at,
            request.expires_at
        ));
        Ok(grant())
    }

    async fn renew(
        &self,
        request: &TaskLeaseRenewRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        self.record(format!(
            "renew:{}:{}:{}",
            request.claim.lease_id, request.observed_at, request.expires_at
        ));
        Ok(grant())
    }

    async fn release(
        &self,
        request: &TaskLeaseCloseRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        self.record(format!(
            "release:{}:{}:{}",
            request.claim.lease_id, request.observed_at, request.reason
        ));
        Ok(grant())
    }

    async fn cancel(
        &self,
        request: &TaskLeaseCloseRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        self.record(format!(
            "cancel:{}:{}:{}",
            request.claim.lease_id, request.observed_at, request.reason
        ));
        Ok(grant())
    }

    async fn validate_claim(
        &self,
        claim: &TaskLeaseClaim,
        observed_at: i64,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        self.record(format!(
            "validate:{}:{}:{}",
            claim.lease_id, claim.fencing_token, observed_at
        ));
        Ok(grant())
    }

    async fn expire_due(&self, observed_at: i64) -> Result<u64, TaskLeaseError> {
        self.record(format!("expire:{observed_at}"));
        Ok(0)
    }
}

#[tokio::test]
async fn task_lease_types_and_port_preserve_p265_contract() {
    let lease_id = LeaseId::new();
    assert!(lease_id.as_str().starts_with("ls_"));
    assert_eq!(lease_id.as_str().len(), 29);
    lease_id.as_str()[3..]
        .parse::<ulid::Ulid>()
        .expect("lease id ULID payload");

    assert!(FencingToken::new(0).is_err());
    let token = FencingToken::new(7).expect("positive token");
    assert_eq!(token.value(), 7);
    assert_eq!(serde_json::to_string(&token).expect("serialize"), "7");
    assert_eq!(
        serde_json::from_str::<FencingToken>("7").expect("deserialize"),
        token
    );
    assert!(serde_json::from_str::<FencingToken>("0").is_err());

    for (status, value, terminal) in [
        (LeaseStatus::Active, "active", false),
        (LeaseStatus::Released, "released", true),
        (LeaseStatus::Expired, "expired", true),
        (LeaseStatus::Cancelled, "cancelled", true),
        (LeaseStatus::Superseded, "superseded", true),
    ] {
        assert_eq!(status.as_str(), value);
        assert_eq!(status.is_terminal(), terminal);
        assert_eq!(LeaseStatus::try_from(value).expect("status parse"), status);
    }

    let expected = grant();
    let claim = expected.claim();
    let dispatch = TaskLeaseDispatchRequest {
        execution_task_id: claim.execution_task_id.clone(),
        worker_incarnation_id: claim.worker_incarnation_id.clone(),
        observed_at: 100,
        expires_at: 200,
    };
    let renew = TaskLeaseRenewRequest {
        claim: claim.clone(),
        observed_at: 150,
        expires_at: 250,
    };
    let close = TaskLeaseCloseRequest {
        claim: claim.clone(),
        observed_at: 160,
        reason: "worker_done".to_string(),
    };

    let port = RecordingTaskLeasePort::default();
    assert_eq!(port.dispatch(&dispatch).await.expect("dispatch"), expected);
    port.renew(&renew).await.expect("renew");
    port.release(&close).await.expect("release");
    port.cancel(&close).await.expect("cancel");
    port.validate_claim(&claim, 170).await.expect("validate");
    assert_eq!(port.expire_due(180).await.expect("expire"), 0);

    assert_eq!(
        port.calls(),
        vec![
            format!(
                "dispatch:{}:{}:100:200",
                claim.execution_task_id, claim.worker_incarnation_id
            ),
            format!("renew:{}:150:250", claim.lease_id),
            format!("release:{}:160:worker_done", claim.lease_id),
            format!("cancel:{}:160:worker_done", claim.lease_id),
            format!("validate:{}:7:170", claim.lease_id),
            "expire:180".to_string(),
        ]
    );
}
