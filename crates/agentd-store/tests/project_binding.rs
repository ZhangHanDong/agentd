//! The project ↔ room ↔ repository binding is an agentd-owned durable record,
//! not a projection of the non-authoritative `projects` locator columns.

use agentd_core::ports::{ProjectBindingError, ProjectBindingPort, ProjectRoomRepoBindingRequest};
use agentd_store::SqliteStore;
use agentd_store::project_binding_repo::SqliteProjectBindingStore;

async fn open_temp() -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect + migrate");
    (store, dir)
}

fn request(project: &str, room: &str, repo: &str) -> ProjectRoomRepoBindingRequest {
    ProjectRoomRepoBindingRequest {
        project_id: project.to_string(),
        room_id: room.to_string(),
        repository_id: repo.to_string(),
        repository_url: format!("https://github.com/example/{repo}.git"),
        default_branch: "main".to_string(),
    }
}

#[tokio::test]
async fn put_binding_creates_then_updates_in_place() {
    let (store, _dir) = open_temp().await;
    let bindings = SqliteProjectBindingStore::new(store.pool().clone());

    let created = bindings
        .put_binding(&request("proj-1", "!room-1:example.org", "agentd"))
        .await
        .expect("create");
    assert_eq!(created.project_id, "proj-1");
    assert_eq!(created.room_id, "!room-1:example.org");
    assert_eq!(created.repository_id, "agentd");
    assert_eq!(created.default_branch, "main");
    assert_eq!(created.record_version, 1);

    let mut moved = request("proj-1", "!room-2:example.org", "agentd");
    moved.default_branch = "trunk".to_string();
    let updated = bindings.put_binding(&moved).await.expect("update");
    assert_eq!(updated.room_id, "!room-2:example.org");
    assert_eq!(updated.default_branch, "trunk");
    assert_eq!(updated.record_version, 2);
    assert_eq!(updated.created_at, created.created_at);

    let by_project = bindings
        .get_binding_for_project("proj-1")
        .await
        .expect("lookup by project");
    assert_eq!(by_project, updated);

    let by_room = bindings
        .get_binding_for_room("!room-2:example.org")
        .await
        .expect("lookup by room");
    assert_eq!(by_room, updated);
}

#[tokio::test]
async fn a_room_cannot_be_bound_to_two_projects() {
    let (store, _dir) = open_temp().await;
    let bindings = SqliteProjectBindingStore::new(store.pool().clone());
    bindings
        .put_binding(&request("proj-1", "!shared:example.org", "agentd"))
        .await
        .expect("first binding");

    let error = bindings
        .put_binding(&request("proj-2", "!shared:example.org", "other"))
        .await
        .expect_err("a room binds to at most one project");
    assert!(matches!(error, ProjectBindingError::Conflict(_)));

    // The first binding is untouched.
    let kept = bindings
        .get_binding_for_room("!shared:example.org")
        .await
        .expect("lookup");
    assert_eq!(kept.project_id, "proj-1");
    assert_eq!(kept.record_version, 1);
}

#[tokio::test]
async fn missing_and_blank_lookups_are_classified() {
    let (store, _dir) = open_temp().await;
    let bindings = SqliteProjectBindingStore::new(store.pool().clone());

    assert!(matches!(
        bindings.get_binding_for_project("ghost").await,
        Err(ProjectBindingError::NotFound(_))
    ));
    assert!(matches!(
        bindings.get_binding_for_room("!ghost:example.org").await,
        Err(ProjectBindingError::NotFound(_))
    ));
    assert!(matches!(
        bindings.get_binding_for_project("  ").await,
        Err(ProjectBindingError::Invalid(_))
    ));
    assert!(matches!(
        bindings
            .put_binding(&request("proj-1", "  ", "agentd"))
            .await,
        Err(ProjectBindingError::Invalid(_))
    ));
    assert!(matches!(
        bindings
            .put_binding(&request("proj-1", "!r:example.org", " "))
            .await,
        Err(ProjectBindingError::Invalid(_))
    ));
}
