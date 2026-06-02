//! P0.9 9a: the production `RunHost` contract, exercised over a REAL `SqliteStore`
//! on a tempfile + the in-memory port fakes (NOT `FakeRunHost`). The full
//! `draft.dot` E2E + emit assertions land in 9a-T3; this skeleton checks
//! construction + a read.

use std::path::PathBuf;

use agentd_bin::{ProductionRunHost, SystemClock};
use agentd_core::test_support::{FakeBackend, MempalStub, RecordingCommandRunner};
use agentd_core::types::RunId;
use agentd_store::SqliteStore;
use agentd_surface::host::RunHost;

fn workflows_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workflows")
}

async fn production_host() -> (ProductionRunHost, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let host = ProductionRunHost::new(
        store,
        Box::new(FakeBackend::new()),
        Box::new(RecordingCommandRunner::new()),
        Box::new(MempalStub::new()),
        Box::new(SystemClock),
        workflows_dir(),
    );
    (host, dir)
}

#[tokio::test]
async fn production_run_snapshot_is_none_for_unknown_run() {
    let (host, _dir) = production_host().await;
    let snap = host
        .run_snapshot(&RunId::from_string("ghost"))
        .await
        .expect("run_snapshot");
    assert!(snap.is_none(), "an unknown run has no snapshot");
}
