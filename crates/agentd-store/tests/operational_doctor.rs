use agentd_store::SqliteStore;
use agentd_store::doctor::OperationalDoctor;

#[tokio::test]
async fn operational_doctor_reports_control_plane_domains_without_raw_logs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");

    let report = OperationalDoctor::new(store.pool().clone())
        .check()
        .await
        .expect("doctor");
    assert_eq!(report.workers_online, 0);
    assert_eq!(report.projects, 0);
    assert_eq!(report.queued_tasks, 0);
    assert_eq!(report.active_leases, 0);
    assert_eq!(report.runtime_resume_pending, 0);
    assert_eq!(report.matrix_rooms, 0);
    assert!(report.ready);
}
