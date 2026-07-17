use std::collections::BTreeMap;
use std::path::PathBuf;

use agentd_core::ports::{RuntimeArchivePort, RuntimeProvider};
use agentd_core::types::{RuntimeAttemptId, RuntimeSessionId};
use agentd_runtime::{ContentAddressedTranscriptStore, ProviderCommand, RuntimeProviderAdapter};

#[test]
fn codex_resume_and_native_reference_are_provider_native() {
    let command = RuntimeProviderAdapter::command(
        &ProviderCommand {
            provider: RuntimeProvider::Codex,
            program: "codex".to_string(),
            arguments: vec!["--quiet".to_string()],
            environment: BTreeMap::new(),
            working_directory: PathBuf::from("/workspace/workspace"),
            custom_resume_arguments: None,
        },
        Some("thread-123"),
    )
    .expect("resume command");
    assert_eq!(command.arguments, ["--quiet", "resume", "thread-123"]);
    assert_eq!(
        RuntimeProviderAdapter::extract_native_session_ref(
            RuntimeProvider::Codex,
            br#"{"thread_id":"thread-123"}"#,
        )
        .as_deref(),
        Some("thread-123")
    );
}

#[test]
fn codex_exec_resume_discards_the_original_prompt_and_preserves_automation_flags() {
    let command = RuntimeProviderAdapter::command(
        &ProviderCommand {
            provider: RuntimeProvider::Codex,
            program: "codex".to_string(),
            arguments: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--ignore-user-config".to_string(),
                "--color".to_string(),
                "never".to_string(),
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
                "--skip-git-repo-check".to_string(),
                "implement the assigned task".to_string(),
            ],
            environment: BTreeMap::new(),
            working_directory: PathBuf::from("/workspace/workspace"),
            custom_resume_arguments: None,
        },
        Some("thread-456"),
    )
    .expect("exec resume command");

    assert_eq!(
        command.arguments,
        [
            "exec",
            "resume",
            "--json",
            "--ignore-user-config",
            "--dangerously-bypass-approvals-and-sandbox",
            "--skip-git-repo-check",
            "thread-456",
        ]
    );
}

#[tokio::test]
async fn transcript_objects_are_content_addressed_and_idempotent() {
    let temporary = tempfile::tempdir().expect("temporary archive");
    let store = ContentAddressedTranscriptStore::new(temporary.path(), 1024).expect("store");
    let session_id = RuntimeSessionId::new();
    let attempt_id = RuntimeAttemptId::new();
    let first = store
        .archive_runtime_transcript(&session_id, &attempt_id, b"redacted", false, 7)
        .await
        .expect("archive");
    let second = store
        .archive_runtime_transcript(&session_id, &attempt_id, b"redacted", false, 8)
        .await
        .expect("archive replay");
    assert_eq!(first.content_sha256, second.content_sha256);
    assert_eq!(first.storage_ref, second.storage_ref);
    assert_eq!(
        std::fs::read(store.object_path(&first.content_sha256)).expect("object"),
        b"redacted"
    );
}
