//! The operator-facing binding API. Statuses follow the project convention:
//! `Invalid` -> 400, `NotFound` -> 404, `Conflict` -> 409, `Unavailable` -> 503.

use agentd_store::SqliteStore;
use agentd_store::project_binding_repo::SqliteProjectBindingStore;
use agentd_surface::http::AuthConfig;
use agentd_surface::project_binding_http::project_binding_router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

async fn app() -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let bindings = Arc::new(SqliteProjectBindingStore::new(store.pool().clone()));
    let mut auth = AuthConfig::open();
    auth.api_token = Some("operator-secret".into());
    (project_binding_router(bindings, auth), dir)
}

async fn send(
    app: axum::Router,
    builder: axum::http::request::Builder,
    body: Option<Value>,
) -> axum::http::Response<Body> {
    let builder = builder.header("authorization", "Bearer operator-secret");
    let request = match body {
        Some(value) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&value).expect("json")))
            .expect("request"),
        None => builder.body(Body::empty()).expect("request"),
    };
    app.oneshot(request).await.expect("response")
}

async fn body_json(response: axum::http::Response<Body>) -> Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

#[tokio::test]
async fn binding_api_declares_reads_and_classifies_errors() {
    let (app, _dir) = app().await;

    let declare = json!({
        "room_id": "!room-1:example.org",
        "repository_id": "agentd",
        "repository_url": "https://github.com/example/agentd.git",
        "default_branch": "main"
    });
    let response = send(
        app.clone(),
        Request::put("/api/projects/proj-1/binding"),
        Some(declare.clone()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let created = body_json(response).await;
    assert_eq!(created["project_id"], "proj-1");
    assert_eq!(created["room_id"], "!room-1:example.org");
    assert_eq!(created["record_version"], 1);

    let response = send(
        app.clone(),
        Request::get("/api/projects/proj-1/binding"),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["repository_id"], "agentd");

    let response = send(
        app.clone(),
        Request::get("/api/rooms/!room-1:example.org/binding"),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["project_id"], "proj-1");

    // NotFound -> 404.
    let response = send(
        app.clone(),
        Request::get("/api/projects/ghost/binding"),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Invalid -> 400.
    let mut blank = declare.clone();
    blank["repository_id"] = json!("   ");
    let response = send(
        app.clone(),
        Request::put("/api/projects/proj-9/binding"),
        Some(blank),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Conflict -> 409: another project claiming the same room.
    let response = send(
        app.clone(),
        Request::put("/api/projects/proj-2/binding"),
        Some(declare),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn binding_api_requires_the_operator_bearer_token() {
    let (app, _dir) = app().await;
    let response = app
        .oneshot(
            Request::get("/api/projects/proj-1/binding")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn binding_put_authenticates_before_reading_the_body() {
    let (app, _dir) = app().await;
    let response = app
        .clone()
        .oneshot(
            Request::put("/api/projects/proj-1/binding")
                .header("content-type", "application/json")
                .body(Body::from("{not json"))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "an unauthenticated caller must never reach body parsing"
    );

    let response = app
        .oneshot(
            Request::put("/api/projects/proj-1/binding")
                .header("authorization", "Bearer operator-secret")
                .header("content-type", "application/json")
                .body(Body::from("{not json"))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "an authenticated caller with a malformed body gets Invalid -> 400"
    );
}

#[tokio::test]
async fn binding_bearer_check_matches_the_shared_operator_helper() {
    let (app, _dir) = app().await;
    let response = app
        .oneshot(
            Request::get("/api/projects/proj-1/binding")
                .header("authorization", "bearer operator-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_ne!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "the shared helper treats the auth scheme case-insensitively"
    );
}
