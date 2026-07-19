use agentd_store::SqliteStore;
use agentd_store::cutover_repo::{self, CutoverPhase};

#[tokio::test]
async fn cutover_state_is_durable_and_transitions_are_fenced() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let pool = store.pool();
    let project = "project-alpha";
    let authority = "authority-r7";

    for phase in [
        CutoverPhase::Observe,
        CutoverPhase::Shadow,
        CutoverPhase::Canary,
        CutoverPhase::Cutover,
        CutoverPhase::Drain,
        CutoverPhase::Retired,
    ] {
        cutover_repo::transition(pool, project, phase, authority, 12, 3)
            .await
            .expect("transition");
    }
    let state = cutover_repo::get(pool, project)
        .await
        .expect("get")
        .expect("state");
    assert_eq!(state.phase, CutoverPhase::Retired);
    assert_eq!(state.matrix_cursor, 12);
    assert!(
        cutover_repo::transition(pool, project, CutoverPhase::Canary, authority, 12, 3)
            .await
            .is_err()
    );
    let rollback = cutover_repo::rollback(pool, project, 4)
        .await
        .expect("rollback");
    assert_eq!(rollback.phase, CutoverPhase::Retired);
    assert_eq!(rollback.lease_epoch, 4);
    assert!(cutover_repo::rollback(pool, project, 4).await.is_err());
    assert!(
        cutover_repo::transition(pool, "", CutoverPhase::Observe, authority, 0, 1)
            .await
            .is_err()
    );
    assert!(
        cutover_repo::transition(
            pool,
            "project-beta",
            CutoverPhase::Observe,
            authority,
            -1,
            1
        )
        .await
        .is_err()
    );
    assert!(
        cutover_repo::transition(
            pool,
            "project-gamma",
            CutoverPhase::Observe,
            authority,
            0,
            0
        )
        .await
        .is_err()
    );
}
