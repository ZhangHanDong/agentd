use std::fs;

use agentd_core::ports::CutoverState;
use agentd_store::{CutoverService, SqliteStore};

fn fixture() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("agent-chat fixture");
    let data = directory.path().join("data");
    fs::create_dir_all(&data).expect("fixture data");
    fs::write(
        data.join("agents.json"),
        r#"{
          "codex-worker": {
            "name": "codex-worker",
            "role": "implementer",
            "type": "codex",
            "online": false
          }
        }"#,
    )
    .expect("agents");
    fs::write(
        data.join("groups.json"),
        r#"{"delivery":{"name":"delivery","members":["codex-worker"]}}"#,
    )
    .expect("groups");
    fs::write(
        data.join("messages.json"),
        r#"[
          {"id":"dm_1","ts":1,"from":"operator","to":"codex-worker","summary":"work","full":"secret body"},
          {"id":"gm_1","ts":2,"from":"operator","group":"delivery","summary":"review","full":"private body","mentions":["codex-worker"]}
        ]"#,
    )
    .expect("messages");
    fs::write(
        data.join("cursors.json"),
        r#"{
          "codex-worker": {
            "inbox": 2,
            "inboxId": "gm_1",
            "groups": {"delivery": 2},
            "groupIds": {"delivery": "gm_1"}
          }
        }"#,
    )
    .expect("cursors");
    fs::write(
        data.join("tasks.json"),
        r#"[{"id":"task_1","status":"completed","priority":"normal","assignee":"codex-worker","labels":[]}]"#,
    )
    .expect("tasks");
    fs::write(
        data.join("task_graphs.json"),
        r#"{"graph_1":{"id":"graph_1","owner":"operator","status":"completed","nodes":[]}}"#,
    )
    .expect("graphs");
    directory
}

#[tokio::test]
async fn final_cutover_reaches_active_only_after_shadow_and_drain() {
    let source = fixture();
    let database = tempfile::tempdir().expect("database directory");
    let store = SqliteStore::connect(database.path().join("agentd.db"))
        .await
        .expect("store");
    let service = CutoverService::new(store);
    let planned = service
        .plan(source.path(), None, 10_000, 1)
        .await
        .expect("plan");
    assert_eq!(planned.state, CutoverState::Planned);

    let imported = service
        .import(&planned.plan.id, source.path(), "import-1", 2)
        .await
        .expect("import");
    assert_eq!(imported.run.state, CutoverState::Shadowing);
    assert_eq!(imported.mapped_records, 7);

    let shadow = service
        .shadow(&planned.plan.id, source.path(), "shadow-1", 3)
        .await
        .expect("shadow");
    assert_eq!(shadow.run.state, CutoverState::Draining);
    assert!(shadow.mismatches.is_empty());
    assert_eq!(shadow.decisions, shadow.matched);

    let drained = service
        .drain(&planned.plan.id, source.path(), "drain-1", 4)
        .await
        .expect("drain");
    assert_eq!(drained.source_inflight, 0);
    assert_eq!(drained.imported_inflight, 0);
    assert_eq!(drained.run.state, CutoverState::HandoffReady);

    service
        .handoff(&planned.plan.id, &[], "handoff-1", 5)
        .await
        .expect("empty project handoff");
    let active = service
        .activate(&planned.plan.id, source.path(), 0, "activate-1", 6)
        .await
        .expect("activate");
    assert_eq!(active.run.state, CutoverState::Active);
    let retired = service
        .retire(&planned.plan.id, "retire-1", 7)
        .await
        .expect("retire");
    assert_eq!(retired.state, CutoverState::Retired);
}

#[tokio::test]
async fn final_cutover_rejects_source_drift_after_plan() {
    let source = fixture();
    let database = tempfile::tempdir().expect("database directory");
    let store = SqliteStore::connect(database.path().join("agentd.db"))
        .await
        .expect("store");
    let service = CutoverService::new(store);
    let planned = service
        .plan(source.path(), None, 10_000, 1)
        .await
        .expect("plan");
    fs::write(source.path().join("data/tasks.json"), "[]").expect("mutate source");
    let error = service
        .import(&planned.plan.id, source.path(), "import-1", 2)
        .await
        .expect_err("source drift must fail");
    assert!(error.to_string().contains("source changed"));
}
