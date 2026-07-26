//! `SQLite` implementation of [`ProjectBindingPort`] over
//! `project_room_repo_bindings` (migration 0025).

use agentd_core::ports::{
    ProjectBindingError, ProjectBindingPort, ProjectRoomRepoBinding, ProjectRoomRepoBindingRequest,
};
use sqlx::{Row, SqlitePool};

use crate::util::now_unix;

#[derive(Debug, Clone)]
pub struct SqliteProjectBindingStore {
    pool: SqlitePool,
}

impl SqliteProjectBindingStore {
    #[must_use]
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn required(value: &str, field: &str) -> Result<String, ProjectBindingError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ProjectBindingError::Invalid(format!("{field} is required")));
    }
    Ok(trimmed.to_string())
}

fn unavailable(error: &sqlx::Error) -> ProjectBindingError {
    ProjectBindingError::Unavailable(error.to_string())
}

fn row_to_binding(row: &sqlx::sqlite::SqliteRow) -> ProjectRoomRepoBinding {
    ProjectRoomRepoBinding {
        project_id: row.get("project_id"),
        room_id: row.get("room_id"),
        repository_id: row.get("repository_id"),
        repository_url: row.get("repository_url"),
        default_branch: row.get("default_branch"),
        record_version: row.get("record_version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

const SELECT_BINDING: &str = "SELECT project_id, room_id, repository_id, repository_url, \
     default_branch, record_version, created_at, updated_at FROM project_room_repo_bindings";

struct BindingColumns<'a> {
    project_id: &'a str,
    room_id: &'a str,
    repository_id: &'a str,
    repository_url: &'a str,
    default_branch: &'a str,
}

/// The write half of `put_binding`, run inside the caller's `BEGIN IMMEDIATE`.
async fn put_binding_in_transaction(
    connection: &mut sqlx::SqliteConnection,
    columns: BindingColumns<'_>,
    now: i64,
) -> Result<ProjectRoomRepoBinding, ProjectBindingError> {
    let BindingColumns {
        project_id,
        room_id,
        repository_id,
        repository_url,
        default_branch,
    } = columns;

    let room_owner: Option<String> =
        sqlx::query_scalar("SELECT project_id FROM project_room_repo_bindings WHERE room_id = ?")
            .bind(room_id)
            .fetch_optional(&mut *connection)
            .await
            .map_err(|e| unavailable(&e))?;
    if room_owner.is_some_and(|owner| owner != project_id) {
        return Err(ProjectBindingError::Conflict(format!(
            "room '{room_id}' is already bound to another project"
        )));
    }

    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT record_version FROM project_room_repo_bindings WHERE project_id = ?",
    )
    .bind(project_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|e| unavailable(&e))?;

    let affected = if existing.is_some() {
        sqlx::query(
            "UPDATE project_room_repo_bindings SET \
              room_id = ?, repository_id = ?, repository_url = ?, default_branch = ?, \
              record_version = record_version + 1, updated_at = ? \
             WHERE project_id = ?",
        )
        .bind(room_id)
        .bind(repository_id)
        .bind(repository_url)
        .bind(default_branch)
        .bind(now)
        .bind(project_id)
        .execute(&mut *connection)
        .await
        .map_err(|e| unavailable(&e))?
    } else {
        sqlx::query(
            "INSERT INTO project_room_repo_bindings \
             (project_id, room_id, repository_id, repository_url, default_branch, \
              record_version, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(project_id)
        .bind(room_id)
        .bind(repository_id)
        .bind(repository_url)
        .bind(default_branch)
        .bind(now)
        .bind(now)
        .execute(&mut *connection)
        .await
        .map_err(|e| unavailable(&e))?
    };
    if affected.rows_affected() != 1 {
        return Err(ProjectBindingError::Conflict(format!(
            "binding for project '{project_id}' changed concurrently"
        )));
    }

    let row = sqlx::query(&format!("{SELECT_BINDING} WHERE project_id = ?"))
        .bind(project_id)
        .fetch_optional(&mut *connection)
        .await
        .map_err(|e| unavailable(&e))?
        .ok_or_else(|| {
            ProjectBindingError::Conflict(format!(
                "binding for project '{project_id}' vanished mid-write"
            ))
        })?;
    Ok(row_to_binding(&row))
}

#[async_trait::async_trait]
impl ProjectBindingPort for SqliteProjectBindingStore {
    async fn put_binding(
        &self,
        request: &ProjectRoomRepoBindingRequest,
    ) -> Result<ProjectRoomRepoBinding, ProjectBindingError> {
        let project_id = required(&request.project_id, "project_id")?;
        let room_id = required(&request.room_id, "room_id")?;
        let repository_id = required(&request.repository_id, "repository_id")?;
        let repository_url = required(&request.repository_url, "repository_url")?;
        let default_branch = required(&request.default_branch, "default_branch")?;
        let now = now_unix();

        let mut connection = self.pool.acquire().await.map_err(|e| unavailable(&e))?;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *connection)
            .await
            .map_err(|e| unavailable(&e))?;

        let result = put_binding_in_transaction(
            &mut connection,
            BindingColumns {
                project_id: &project_id,
                room_id: &room_id,
                repository_id: &repository_id,
                repository_url: &repository_url,
                default_branch: &default_branch,
            },
            now,
        )
        .await;

        match result {
            Ok(binding) => {
                sqlx::query("COMMIT")
                    .execute(&mut *connection)
                    .await
                    .map_err(|e| unavailable(&e))?;
                Ok(binding)
            }
            Err(error) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                Err(error)
            }
        }
    }

    async fn get_binding_for_project(
        &self,
        project_id: &str,
    ) -> Result<ProjectRoomRepoBinding, ProjectBindingError> {
        let project_id = required(project_id, "project_id")?;
        sqlx::query(&format!("{SELECT_BINDING} WHERE project_id = ?"))
            .bind(&project_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| unavailable(&e))?
            .as_ref()
            .map(row_to_binding)
            .ok_or_else(|| {
                ProjectBindingError::NotFound(format!("no binding for project '{project_id}'"))
            })
    }

    async fn get_binding_for_room(
        &self,
        room_id: &str,
    ) -> Result<ProjectRoomRepoBinding, ProjectBindingError> {
        let room_id = required(room_id, "room_id")?;
        sqlx::query(&format!("{SELECT_BINDING} WHERE room_id = ?"))
            .bind(&room_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| unavailable(&e))?
            .as_ref()
            .map(row_to_binding)
            .ok_or_else(|| {
                ProjectBindingError::NotFound(format!("no binding for room '{room_id}'"))
            })
    }
}
