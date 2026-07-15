use agentd_store::{SqliteStore, agent_chat_task_repo};

async fn open_store() -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("target dir");
    let db = dir.path().join("agentd.db");
    let store = SqliteStore::connect(&db).await.expect("connect store");
    (store, dir)
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn agent_chat_live_task_repo_crud_filters_transitions_and_comments() {
    let (store, _dir) = open_store().await;

    let created = agent_chat_task_repo::create_task(
        store.pool(),
        agent_chat_task_repo::CreateAgentChatTask {
            title: "  Fix live task parity  ".to_string(),
            description: Some("Implement /api/tasks".to_string()),
            priority: Some("p1".to_string()),
            granularity: None,
            assignee: Some("codex-a".to_string()),
            created_by: Some("alex".to_string()),
            parent_id: None,
            labels: vec![
                "migration".to_string(),
                "migration".to_string(),
                "http".to_string(),
            ],
        },
    )
    .await
    .expect("create task");

    assert!(created.id.starts_with("task_"));
    assert_eq!(created.title, "Fix live task parity");
    assert_eq!(created.description, "Implement /api/tasks");
    assert_eq!(created.status, "created");
    assert_eq!(created.priority, "p1");
    assert_eq!(created.granularity, "task");
    assert_eq!(created.assignee.as_deref(), Some("codex-a"));
    assert_eq!(created.labels, vec!["migration", "http"]);
    assert!(created.health.is_none());
    assert!(created.comments.is_empty());

    agent_chat_task_repo::create_task(
        store.pool(),
        agent_chat_task_repo::CreateAgentChatTask {
            title: "Review live task parity".to_string(),
            description: None,
            priority: Some("p2".to_string()),
            granularity: Some("task".to_string()),
            assignee: Some("codex-b".to_string()),
            created_by: None,
            parent_id: None,
            labels: vec!["review".to_string()],
        },
    )
    .await
    .expect("create second task");

    let filtered = agent_chat_task_repo::list_tasks(
        store.pool(),
        agent_chat_task_repo::AgentChatTaskFilters {
            assignee: Some("codex-a".to_string()),
            statuses: vec!["created".to_string(), "blocked".to_string()],
            priority: Some("p1".to_string()),
            label: Some("http".to_string()),
            offset: 0,
            limit: Some(10),
        },
    )
    .await
    .expect("list filtered tasks");
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id, created.id);

    let patched = agent_chat_task_repo::update_task(
        store.pool(),
        &created.id,
        agent_chat_task_repo::UpdateAgentChatTask {
            title: Some("Updated live task parity".to_string()),
            description: Some("Still agent-chat compatible".to_string()),
            priority: Some("p0".to_string()),
            granularity: None,
            assignee: Some(Some("codex-a".to_string())),
            labels: Some(vec!["updated".to_string()]),
            parent_id: None,
        },
    )
    .await
    .expect("patch task")
    .expect("task exists");
    assert_eq!(patched.title, "Updated live task parity");
    assert_eq!(patched.priority, "p0");
    assert_eq!(patched.labels, vec!["updated"]);

    let execution = agent_chat_task_repo::update_task_execution(
        store.pool(),
        &created.id,
        agent_chat_task_repo::UpdateAgentChatTaskExecution {
            heartbeat_at: Some(true),
            waiting_reason: Some(Some("waiting for CI".to_string())),
            waiting_until: None,
        },
    )
    .await
    .expect("execution update")
    .expect("task exists");
    assert!(execution.heartbeat_at.is_some());
    assert_eq!(execution.waiting_reason.as_deref(), Some("waiting for CI"));
    assert_eq!(execution.status, "created");

    let accepted = agent_chat_task_repo::transition_task(
        store.pool(),
        &created.id,
        agent_chat_task_repo::TransitionAgentChatTask {
            status: "accepted".to_string(),
            waiting_reason: None,
            waiting_until: None,
        },
    )
    .await
    .expect("accept task")
    .expect("task exists");
    assert_eq!(accepted.status, "accepted");
    assert!(accepted.started_at.is_some());

    let started = agent_chat_task_repo::transition_task(
        store.pool(),
        &created.id,
        agent_chat_task_repo::TransitionAgentChatTask {
            status: "in_progress".to_string(),
            waiting_reason: None,
            waiting_until: None,
        },
    )
    .await
    .expect("start task")
    .expect("task exists");
    assert_eq!(started.status, "in_progress");

    let blocked = agent_chat_task_repo::transition_task(
        store.pool(),
        &created.id,
        agent_chat_task_repo::TransitionAgentChatTask {
            status: "blocked".to_string(),
            waiting_reason: Some("waiting for deploy".to_string()),
            waiting_until: Some("2026-07-10T00:00:00Z".to_string()),
        },
    )
    .await
    .expect("block task")
    .expect("task exists");
    assert_eq!(blocked.status, "blocked");
    assert_eq!(
        blocked.waiting_reason.as_deref(),
        Some("waiting for deploy")
    );

    let invalid = agent_chat_task_repo::transition_task(
        store.pool(),
        &created.id,
        agent_chat_task_repo::TransitionAgentChatTask {
            status: "done".to_string(),
            waiting_reason: None,
            waiting_until: None,
        },
    )
    .await
    .expect_err("blocked -> done is rejected");
    assert!(
        invalid.to_string().contains("cannot transition"),
        "invalid transition error: {invalid}"
    );

    let resumed = agent_chat_task_repo::transition_task(
        store.pool(),
        &created.id,
        agent_chat_task_repo::TransitionAgentChatTask {
            status: "in_progress".to_string(),
            waiting_reason: None,
            waiting_until: None,
        },
    )
    .await
    .expect("resume task")
    .expect("task exists");
    assert_eq!(resumed.status, "in_progress");
    assert!(resumed.waiting_reason.is_none());
    assert!(resumed.waiting_until.is_none());

    let commented = agent_chat_task_repo::add_comment(
        store.pool(),
        &created.id,
        agent_chat_task_repo::AddAgentChatTaskComment {
            author: Some("operator".to_string()),
            text: "looks good".to_string(),
        },
    )
    .await
    .expect("add comment")
    .expect("task exists");
    assert_eq!(commented.comments.len(), 1);
    assert_eq!(commented.comments[0].author, "operator");
    assert_eq!(commented.comments[0].text, "looks good");

    let deleted = agent_chat_task_repo::delete_task(store.pool(), &created.id)
        .await
        .expect("delete task")
        .expect("deleted task");
    assert_eq!(deleted.id, created.id);
    assert!(
        agent_chat_task_repo::get_task(store.pool(), &created.id)
            .await
            .expect("get deleted task")
            .is_none()
    );
}
