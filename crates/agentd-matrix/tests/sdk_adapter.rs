use std::path::PathBuf;
use std::process::Command;

#[cfg(feature = "matrix-sdk-adapter")]
use agentd_matrix::{
    BridgeError, MatrixClientPort, SdkMatrixClient, SdkMatrixClientConfig,
    sdk_timeline_text_messages,
};
#[cfg(feature = "matrix-sdk-adapter")]
use matrix_sdk::{
    deserialized_responses::TimelineEvent,
    ruma::{events::AnySyncTimelineEvent, serde::Raw},
};
#[cfg(feature = "matrix-sdk-adapter")]
use serde_json::{Value, json};

fn repo_root() -> PathBuf {
    let mut path = std::env::current_dir().expect("read current test directory");
    loop {
        let manifest = path.join("Cargo.toml");
        if std::fs::read_to_string(&manifest).is_ok_and(|content| {
            content.contains("[workspace]") && content.contains("\"crates/agentd-matrix\"")
        }) {
            return path;
        }
        assert!(
            path.pop(),
            "find agentd workspace root from current test directory"
        );
    }
}

fn crate_root() -> PathBuf {
    repo_root().join("crates/agentd-matrix")
}

#[cfg(feature = "matrix-sdk-adapter")]
fn sync_timeline_event(value: Value) -> TimelineEvent {
    let raw: Raw<AnySyncTimelineEvent> = serde_json::from_value(value).expect("raw timeline event");
    TimelineEvent::from_plaintext(raw)
}

#[cfg(not(feature = "matrix-sdk-adapter"))]
fn run_feature_subtest(filter: &str) {
    let output = Command::new("cargo")
        .args([
            "test",
            "-p",
            "agentd-matrix",
            "--test",
            "sdk_adapter",
            "--features",
            "matrix-sdk-adapter",
            filter,
        ])
        .current_dir(repo_root())
        .output()
        .expect("run feature-gated SDK adapter subtest");

    assert!(
        output.status.success(),
        "feature-gated SDK adapter subtest {filter} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn sdk_adapter_feature_is_opt_in_and_default_build_stays_sdk_free() {
    let manifest = std::fs::read_to_string(crate_root().join("Cargo.toml"))
        .expect("read agentd-matrix manifest");

    assert!(
        manifest.contains("[features]"),
        "agentd-matrix manifest should declare features"
    );
    assert!(
        manifest.contains("matrix-sdk-adapter"),
        "agentd-matrix should expose the matrix-sdk-adapter feature"
    );
    assert!(
        manifest.contains("matrix-sdk = { workspace = true")
            && manifest.contains("optional = true")
            && manifest.contains("rustls-tls"),
        "matrix-sdk should be an optional dependency"
    );
    assert!(
        manifest.contains("default = []"),
        "default features should stay empty"
    );
}

#[test]
fn secure_matrix_storage_dependency_baseline_is_pinned() {
    let root = repo_root();
    let manifest =
        std::fs::read_to_string(root.join("Cargo.toml")).expect("read workspace Cargo manifest");
    let authority_manifest =
        std::fs::read_to_string(root.join("crates/agentd-project-authority/Cargo.toml"))
            .expect("read project authority Cargo manifest");
    let clippy = std::fs::read_to_string(root.join("clippy.toml")).expect("read Clippy config");
    let readme = std::fs::read_to_string(root.join("README.md")).expect("read README");

    for expected in [
        "rust-version = \"1.94\"",
        "sqlx = { version = \"0.9\"",
        "matrix-sdk = { version = \"0.16.1\"",
        "features = [\"e2e-encryption\", \"sqlite\"]",
    ] {
        assert!(
            manifest.contains(expected),
            "workspace dependency baseline should contain {expected}"
        );
    }
    assert!(
        clippy.contains("msrv = \"1.94\""),
        "Clippy should evaluate lints against the workspace MSRV"
    );
    assert!(
        readme.contains("MSRV 1.94"),
        "README should publish the current workspace MSRV"
    );
    assert!(
        authority_manifest.contains("publish = false"),
        "the internal project-authority crate should remain unpublished"
    );
}

#[test]
fn dependency_governance_exceptions_are_scoped() {
    let deny =
        std::fs::read_to_string(repo_root().join("deny.toml")).expect("read cargo-deny config");

    for expected in [
        "yanked = \"deny\"",
        "id = \"RUSTSEC-2026-0173\"",
        "crate = \"webpki-roots@1\"",
        "crate = \"xxhash-rust@0.8\"",
    ] {
        assert!(
            deny.contains(expected),
            "dependency governance should contain scoped entry {expected}"
        );
    }
    assert_eq!(
        deny.matches("reason = ").count(),
        1,
        "each advisory or yanked exception should record one reason"
    );
}

#[test]
fn sdk_matrix_client_config_validates_homeserver_and_credentials() {
    #[cfg(not(feature = "matrix-sdk-adapter"))]
    {
        run_feature_subtest("sdk_matrix_client_config_validates_homeserver_and_credentials");
    }

    #[cfg(feature = "matrix-sdk-adapter")]
    {
        let password_login = SdkMatrixClientConfig::new("https://matrix.example.test")
            .with_password_login("agent-bridge", "secret");
        password_login.validate().expect("password login config");

        let token_restore = SdkMatrixClientConfig::new("https://matrix.example.test")
            .with_access_token("@agent-bridge:matrix.example.test", "token");
        token_restore.validate().expect("token restore config");

        let empty_homeserver = SdkMatrixClientConfig::new(" ");
        assert!(
            empty_homeserver.validate().is_err(),
            "empty homeserver URL must fail validation"
        );

        let mixed = SdkMatrixClientConfig::new("https://matrix.example.test")
            .with_password_login("agent-bridge", "secret")
            .with_access_token("@agent-bridge:matrix.example.test", "token");
        assert!(
            mixed.validate().is_err(),
            "password login and token restore must be mutually exclusive"
        );
    }
}

#[test]
fn sdk_matrix_client_builds_local_client_from_direct_homeserver_url() {
    #[cfg(not(feature = "matrix-sdk-adapter"))]
    {
        run_feature_subtest("sdk_matrix_client_builds_local_client_from_direct_homeserver_url");
    }

    #[cfg(feature = "matrix-sdk-adapter")]
    {
        let config = SdkMatrixClientConfig::new("https://matrix.example.test");
        let client = SdkMatrixClient::build(config).expect("build local SDK adapter");

        assert_eq!(client.homeserver_url(), "https://matrix.example.test");
    }
}

#[test]
fn sdk_matrix_client_sqlite_store_reopens_persisted_state() {
    #[cfg(not(feature = "matrix-sdk-adapter"))]
    {
        run_feature_subtest("sdk_matrix_client_sqlite_store_reopens_persisted_state");
    }

    #[cfg(feature = "matrix-sdk-adapter")]
    {
        let temp = tempfile::tempdir().expect("create Matrix SDK store tempdir");
        let store_path = temp.path().join("matrix-sdk");
        let key = b"agentd-p155-state";
        let value = b"persisted-across-client-reopen".to_vec();

        {
            let config = SdkMatrixClientConfig::new("https://matrix.example.test")
                .with_sqlite_store_path(&store_path);
            let client = SdkMatrixClient::build(config).expect("open Matrix SDK SQLite store");
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build state-store runtime");
            runtime
                .block_on(
                    client
                        .sdk_client()
                        .state_store()
                        .set_custom_value_no_read(key, value.clone()),
                )
                .expect("persist Matrix SDK state");
        }

        let config = SdkMatrixClientConfig::new("https://matrix.example.test")
            .with_sqlite_store_path(&store_path);
        let client = SdkMatrixClient::build(config).expect("reopen Matrix SDK SQLite store");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build state-store runtime");
        let persisted = runtime
            .block_on(client.sdk_client().state_store().get_custom_value(key))
            .expect("read persisted Matrix SDK state");

        assert_eq!(persisted, Some(value));
    }
}

#[test]
fn sdk_matrix_client_sync_path_is_source_bound_to_matrix_sdk() {
    let source = std::fs::read_to_string(crate_root().join("src/lib.rs")).expect("read lib.rs");

    for expected in [
        "matrix_sdk::Client",
        ".sync_once(",
        ".joined_rooms()",
        ".invited_rooms()",
        "sdk_timeline_text_messages",
        "timeline.events",
    ] {
        assert!(
            source.contains(expected),
            "SDK adapter source should contain {expected}"
        );
    }
}

#[test]
fn sdk_matrix_client_maps_room_lookup_errors_without_network() {
    #[cfg(not(feature = "matrix-sdk-adapter"))]
    {
        run_feature_subtest("sdk_matrix_client_maps_room_lookup_errors_without_network");
    }

    #[cfg(feature = "matrix-sdk-adapter")]
    {
        let config = SdkMatrixClientConfig::new("https://matrix.example.test");
        let mut client = SdkMatrixClient::build(config).expect("build local SDK adapter");

        let invalid = client
            .leave_room("not-a-room-id")
            .expect_err("invalid room id");
        assert!(
            matches!(invalid, BridgeError::Transport(_)),
            "invalid room id should map to a transport error"
        );
        assert!(
            invalid.to_string().contains("not-a-room-id"),
            "invalid room id should be visible in the error: {invalid}"
        );

        let unknown = client
            .send_text_message("!unknown:matrix.example.test", "hello")
            .expect_err("unknown room id");
        assert!(
            matches!(unknown, BridgeError::Transport(_)),
            "unknown room should map to a transport error"
        );
        assert!(
            unknown.to_string().contains("!unknown:matrix.example.test"),
            "unknown room id should be visible in the error: {unknown}"
        );
    }
}

#[test]
fn sdk_timeline_parser_extracts_text_mentions_and_reply_from_raw_sync_events() {
    #[cfg(not(feature = "matrix-sdk-adapter"))]
    {
        run_feature_subtest(
            "sdk_timeline_parser_extracts_text_mentions_and_reply_from_raw_sync_events",
        );
    }

    #[cfg(feature = "matrix-sdk-adapter")]
    {
        let events = vec![
            sync_timeline_event(json!({
                "type": "m.room.message",
                "event_id": "$plain:matrix.test",
                "sender": "@alex:matrix.test",
                "origin_server_ts": 1,
                "content": {
                    "msgtype": "m.text",
                    "body": "hello codex",
                    "m.mentions": {
                        "user_ids": ["@codex-worker:matrix.test"]
                    }
                }
            })),
            sync_timeline_event(json!({
                "type": "m.room.message",
                "event_id": "$reply:matrix.test",
                "sender": "@reviewer:matrix.test",
                "origin_server_ts": 2,
                "content": {
                    "msgtype": "m.notice",
                    "body": "reply body",
                    "m.relates_to": {
                        "m.in_reply_to": {
                            "event_id": "$plain:matrix.test"
                        }
                    }
                }
            })),
        ];

        let parsed = sdk_timeline_text_messages("!ops:matrix.test", &events);

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].event_id, "$plain:matrix.test");
        assert_eq!(parsed[0].room_id, "!ops:matrix.test");
        assert_eq!(parsed[0].sender_mxid, "@alex:matrix.test");
        assert_eq!(parsed[0].body, "hello codex");
        assert_eq!(parsed[0].mentions, vec!["@codex-worker:matrix.test"]);
        assert_eq!(parsed[0].reply_to, None);
        assert_eq!(parsed[1].event_id, "$reply:matrix.test");
        assert_eq!(parsed[1].body, "reply body");
        assert_eq!(parsed[1].reply_to.as_deref(), Some("$plain:matrix.test"));
    }
}

#[test]
fn sdk_timeline_parser_preserves_formatted_body_for_bot_command_ingress() {
    #[cfg(not(feature = "matrix-sdk-adapter"))]
    {
        run_feature_subtest("sdk_timeline_parser_preserves_formatted_body_for_bot_command_ingress");
    }

    #[cfg(feature = "matrix-sdk-adapter")]
    {
        let formatted_body =
            r#"<a href="https://matrix.to/#/@agent-bridge:matrix.test">Agent Bridge</a>: !status"#;
        let events = vec![sync_timeline_event(json!({
            "type": "m.room.message",
            "event_id": "$formatted:matrix.test",
            "sender": "@alex:matrix.test",
            "origin_server_ts": 3,
            "content": {
                "msgtype": "m.text",
                "body": "Agent Bridge: !status",
                "format": "org.matrix.custom.html",
                "formatted_body": formatted_body
            }
        }))];

        let parsed = sdk_timeline_text_messages("!ops:matrix.test", &events);

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].body, "Agent Bridge: !status");
        assert_eq!(parsed[0].formatted_body.as_deref(), Some(formatted_body));
    }
}

#[test]
fn sdk_timeline_parser_skips_state_redacted_non_message_and_malformed_events() {
    #[cfg(not(feature = "matrix-sdk-adapter"))]
    {
        run_feature_subtest(
            "sdk_timeline_parser_skips_state_redacted_non_message_and_malformed_events",
        );
    }

    #[cfg(feature = "matrix-sdk-adapter")]
    {
        let events = vec![
            sync_timeline_event(json!({
                "type": "m.room.name",
                "event_id": "$state:matrix.test",
                "sender": "@alex:matrix.test",
                "origin_server_ts": 1,
                "state_key": "",
                "content": {
                    "name": "Ops"
                }
            })),
            sync_timeline_event(json!({
                "type": "m.room.message",
                "event_id": "$redacted:matrix.test",
                "sender": "@alex:matrix.test",
                "origin_server_ts": 2,
                "content": {}
            })),
            sync_timeline_event(json!({
                "type": "m.reaction",
                "event_id": "$reaction:matrix.test",
                "sender": "@alex:matrix.test",
                "origin_server_ts": 3,
                "content": {
                    "m.relates_to": {
                        "rel_type": "m.annotation",
                        "event_id": "$kept:matrix.test",
                        "key": "+1"
                    }
                }
            })),
            sync_timeline_event(json!({
                "type": "m.room.message",
                "content": {
                    "msgtype": "m.text",
                    "body": "missing event metadata"
                }
            })),
            sync_timeline_event(json!({
                "type": "m.room.message",
                "event_id": "$kept:matrix.test",
                "sender": "@codex-worker:matrix.test",
                "origin_server_ts": 4,
                "content": {
                    "msgtype": "m.emote",
                    "body": "waves"
                }
            })),
        ];

        let parsed = sdk_timeline_text_messages("!ops:matrix.test", &events);

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].event_id, "$kept:matrix.test");
        assert_eq!(parsed[0].sender_mxid, "@codex-worker:matrix.test");
        assert_eq!(parsed[0].body, "waves");
    }
}

#[test]
fn sdk_matrix_client_sync_path_uses_sync_response_timeline_events() {
    let source = std::fs::read_to_string(crate_root().join("src/lib.rs")).expect("read lib.rs");

    for expected in [
        "let sync = self",
        "sync.rooms.join",
        "timeline.events",
        "sdk_timeline_text_messages",
    ] {
        assert!(
            source.contains(expected),
            "SDK adapter sync source should contain {expected}"
        );
    }
    assert!(
        !source.contains("text_events: Vec::new()"),
        "SDK adapter sync path should no longer hard-code empty text events"
    );
}

#[test]
fn sdk_timeline_parser_stays_feature_gated_in_default_build() {
    let manifest = std::fs::read_to_string(crate_root().join("Cargo.toml")).expect("read manifest");
    let source = std::fs::read_to_string(crate_root().join("src/lib.rs")).expect("read source");

    assert!(
        manifest.contains("default = []"),
        "default features should remain empty"
    );
    assert!(
        source.contains(
            "#[cfg(feature = \"matrix-sdk-adapter\")]\npub fn sdk_timeline_text_messages"
        ),
        "SDK timeline parser should be feature-gated"
    );
}

#[test]
fn sdk_adapter_feature_path_compiles_with_matrix_sdk_enabled() {
    if std::env::var_os("AGENTD_SKIP_NESTED_CARGO_CHECK").is_some() {
        return;
    }

    let output = Command::new("cargo")
        .args([
            "check",
            "-p",
            "agentd-matrix",
            "--features",
            "matrix-sdk-adapter",
            "--tests",
        ])
        .env("AGENTD_SKIP_NESTED_CARGO_CHECK", "1")
        .current_dir(repo_root())
        .output()
        .expect("run cargo check");

    assert!(
        output.status.success(),
        "cargo check with matrix-sdk-adapter failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn sdk_adapter_feature_path_compiles_dm_room_lifecycle_methods() {
    if std::env::var_os("AGENTD_SKIP_NESTED_CARGO_CHECK").is_some() {
        return;
    }

    let source = std::fs::read_to_string(crate_root().join("src/lib.rs")).expect("read source");
    for expected in [
        "fn room_member_status(",
        "fn create_direct_room(",
        "fn invite_user_to_room(",
        "create_room::v3::RoomPreset::TrustedPrivateChat",
        "MatrixBotDmRoomStatus::InviteFailed",
    ] {
        assert!(
            source.contains(expected),
            "SDK DM lifecycle source should mention {expected}"
        );
    }

    let output = Command::new("cargo")
        .args([
            "check",
            "-p",
            "agentd-matrix",
            "--features",
            "matrix-sdk-adapter",
            "--tests",
        ])
        .env("AGENTD_SKIP_NESTED_CARGO_CHECK", "1")
        .current_dir(repo_root())
        .output()
        .expect("run cargo check");

    assert!(
        output.status.success(),
        "feature-gated SDK adapter DM lifecycle cargo check failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
