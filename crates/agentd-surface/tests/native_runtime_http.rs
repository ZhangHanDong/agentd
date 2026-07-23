//! AD-E5: HTTP transport for the native runtime control-plane port.

use std::sync::Arc;

use agentd_core::ports::{
    NativeRuntimeAttemptStart, NativeRuntimeAttemptState, NativeRuntimeControlError,
    NativeRuntimeControlPort,
};
use agentd_core::types::{
    RunId, RuntimeAttemptId, RuntimeAttemptStatus, RuntimeSessionId, TaskRunId, WorkerIncarnationId,
};
use agentd_surface::http::AuthConfig;
use agentd_surface::native_runtime_http::native_runtime_router;
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;

/// Fake control plane returning scripted results per operation.
#[derive(Debug, Clone, Default)]
struct FakeControlPlane {
    validate_error: Option<NativeRuntimeControlError>,
    start_error: Option<NativeRuntimeControlError>,
}

#[async_trait]
impl NativeRuntimeControlPort for FakeControlPlane {
    async fn validate_session_task(
        &self,
        _session_id: &RuntimeSessionId,
        _task_id: &TaskRunId,
    ) -> Result<(), NativeRuntimeControlError> {
        match &self.validate_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    async fn session_view(
        &self,
        session_id: &RuntimeSessionId,
    ) -> Result<Option<agentd_core::ports::NativeRuntimeSessionView>, NativeRuntimeControlError>
    {
        Ok(Some(agentd_core::ports::NativeRuntimeSessionView {
            session_id: session_id.clone(),
            task_id: TaskRunId::new(),
            run_id: RunId::new(),
            status: agentd_core::types::RuntimeSessionStatus::ResumePending,
            latest_native_session_ref: Some("thread-view-1".to_string()),
            snapshot: agentd_core::ports::ExecutionSnapshotLink::default(),
        }))
    }

    async fn session_for_task(
        &self,
        _task_id: &TaskRunId,
    ) -> Result<Option<agentd_core::ports::NativeRuntimeSessionView>, NativeRuntimeControlError>
    {
        Ok(None)
    }

    async fn start_attempt(
        &self,
        request: &NativeRuntimeAttemptStart,
    ) -> Result<NativeRuntimeAttemptState, NativeRuntimeControlError> {
        match &self.start_error {
            Some(error) => Err(error.clone()),
            None => Ok(NativeRuntimeAttemptState {
                attempt_id: request.attempt_id.clone(),
                session_id: request.session_id.clone(),
                status: RuntimeAttemptStatus::Starting,
                native_session_ref: None,
                exit_code: None,
                observed_at: request.observed_at,
            }),
        }
    }

    async fn update_attempt(
        &self,
        _state: &NativeRuntimeAttemptState,
    ) -> Result<(), NativeRuntimeControlError> {
        Ok(())
    }

    async fn mark_attempt_terminal(
        &self,
        state: &NativeRuntimeAttemptState,
    ) -> Result<(), NativeRuntimeControlError> {
        if !matches!(
            state.status,
            RuntimeAttemptStatus::Exited | RuntimeAttemptStatus::Gone
        ) {
            return Err(NativeRuntimeControlError::Invalid(
                "terminal state must be exited or gone".into(),
            ));
        }
        Ok(())
    }
}

fn auth() -> AuthConfig {
    AuthConfig {
        api_token: Some("worker-secret".to_string()),
        ..AuthConfig::default()
    }
}

fn start_body() -> serde_json::Value {
    json!({
        "attempt_id": RuntimeAttemptId::new(),
        "session_id": RuntimeSessionId::new(),
        "task_id": TaskRunId::new(),
        "worker_incarnation_id": WorkerIncarnationId::new(),
        "observed_at": 1
    })
}

fn post(path: &str, token: Option<&str>, body: &serde_json::Value) -> Request<Body> {
    let mut request = Request::post(path).header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    request
        .body(Body::from(serde_json::to_vec(body).expect("json")))
        .expect("request")
}

#[tokio::test]
async fn native_runtime_routes_require_bearer_token() {
    let app = native_runtime_router(Arc::new(FakeControlPlane::default()), auth());
    let response = app
        .oneshot(post(
            "/api/runtime/native/attempt/start",
            None,
            &start_body(),
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn native_runtime_start_attempt_returns_control_plane_state() {
    let app = native_runtime_router(Arc::new(FakeControlPlane::default()), auth());
    let body = start_body();
    let response = app
        .oneshot(post(
            "/api/runtime/native/attempt/start",
            Some("worker-secret"),
            &body,
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let state: serde_json::Value = serde_json::from_slice(&bytes).expect("state json");
    assert_eq!(state["attempt_id"], body["attempt_id"]);
    assert_eq!(state["session_id"], body["session_id"]);
    assert_eq!(state["status"], "starting");
}

#[tokio::test]
async fn native_runtime_validate_returns_ok_envelope() {
    let app = native_runtime_router(Arc::new(FakeControlPlane::default()), auth());
    let body = json!({
        "session_id": RuntimeSessionId::new(),
        "task_id": TaskRunId::new()
    });
    let response = app
        .oneshot(post(
            "/api/runtime/native/session/validate",
            Some("worker-secret"),
            &body,
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn native_runtime_errors_map_to_retryable_statuses() {
    let cases = [
        (
            NativeRuntimeControlError::Invalid("bad".into()),
            StatusCode::BAD_REQUEST,
        ),
        (
            NativeRuntimeControlError::NotFound("missing".into()),
            StatusCode::NOT_FOUND,
        ),
        (
            NativeRuntimeControlError::Conflict("fenced".into()),
            StatusCode::CONFLICT,
        ),
        (
            NativeRuntimeControlError::Unavailable("db down".into()),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ];
    for (error, expected) in cases {
        let app = native_runtime_router(
            Arc::new(FakeControlPlane {
                start_error: Some(error.clone()),
                validate_error: None,
            }),
            auth(),
        );
        let response = app
            .oneshot(post(
                "/api/runtime/native/attempt/start",
                Some("worker-secret"),
                &start_body(),
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), expected, "error {error:?}");
    }
}

#[tokio::test]
async fn native_runtime_session_view_returns_resume_reference() {
    let app = native_runtime_router(Arc::new(FakeControlPlane::default()), auth());
    let body = json!({ "session_id": RuntimeSessionId::new() });
    let response = app
        .oneshot(post(
            "/api/runtime/native/session/view",
            Some("worker-secret"),
            &body,
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let view: serde_json::Value = serde_json::from_slice(&bytes).expect("view json");
    assert_eq!(view["session_id"], body["session_id"]);
    assert_eq!(view["status"], "resume_pending");
    assert_eq!(view["latest_native_session_ref"], "thread-view-1");
}

#[tokio::test]
async fn native_runtime_terminal_rejects_non_terminal_status() {
    let app = native_runtime_router(Arc::new(FakeControlPlane::default()), auth());
    let body = json!({
        "attempt_id": RuntimeAttemptId::new(),
        "session_id": RuntimeSessionId::new(),
        "status": "running",
        "native_session_ref": null,
        "observed_at": 5
    });
    let response = app
        .oneshot(post(
            "/api/runtime/native/attempt/terminal",
            Some("worker-secret"),
            &body,
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
