//! Smoke wiring: open the real `SQLite` store (creating + migrating it), seed a
//! project, and report. Proves the daemon can stand up its local state.
//!
//! Run with: `AGENTD_HOME=/tmp/agentd-smoke cargo run -p agentd-bin --example wire_real_store`
//! (honors `AGENTD_HOME` so it need not touch your real `~/.agentd`).

use agentd_store::{SqliteStore, paths, project_repo};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = paths::default_db_path();
    let store = SqliteStore::connect(&db_path).await?;
    project_repo::insert_project(
        store.pool(),
        "smoke-project",
        "smoke",
        "/tmp/smoke-repo",
        "smoke-wing",
    )
    .await?;
    let projects = project_repo::count_projects(store.pool()).await?;
    println!(
        "agentd store ready at {} ({projects} project(s))",
        db_path.display()
    );
    Ok(())
}
