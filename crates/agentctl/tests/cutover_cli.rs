use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn agentctl() -> Command {
    Command::new(env!("CARGO_BIN_EXE_agentctl"))
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, content).expect("fixture file");
}

fn source_fixture() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("source fixture");
    let data = directory.path().join("data");
    write(
        &data.join("agents.json"),
        r#"{"codex-worker":{"name":"codex-worker","role":"implementer","type":"codex","online":false}}"#,
    );
    write(&data.join("groups.json"), "{}");
    write(&data.join("messages.json"), "[]");
    write(&data.join("cursors.json"), "{}");
    write(&data.join("tasks.json"), "[]");
    write(&data.join("task_graphs.json"), "{}");
    directory
}

fn stdout_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON output")
}

fn plan(source: &Path, database: &Path) -> Value {
    let output = agentctl()
        .args([
            "cutover",
            "plan",
            "--agent-chat",
            source.to_str().expect("source path"),
            "--db-path",
            database.to_str().expect("database path"),
        ])
        .output()
        .expect("run cutover plan");
    stdout_json(&output)
}

#[test]
fn cutover_help_exposes_the_complete_operator_surface() {
    let output = agentctl()
        .args(["cutover", "--help"])
        .output()
        .expect("cutover help");
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("UTF-8 help");
    for command in [
        "plan",
        "import",
        "shadow",
        "drain",
        "handoff",
        "activate",
        "retire",
        "inspect",
        "rollback",
        "doctor",
        "backup",
        "restore",
        "service-install",
    ] {
        assert!(help.contains(command), "missing command {command}");
    }
}

#[test]
fn backup_manifest_and_offline_restore_are_digest_verified() {
    let source = source_fixture();
    let directory = tempfile::tempdir().expect("database directory");
    let database = directory.path().join("agentd.db");
    let planned = plan(source.path(), &database);
    let cutover_id = planned["plan"]["id"].as_str().expect("cutover id");
    let backup = directory.path().join("backups/agentd.db");

    let backup_output = agentctl()
        .args([
            "cutover",
            "backup",
            cutover_id,
            "--db-path",
            database.to_str().expect("database path"),
            "--output",
            backup.to_str().expect("backup path"),
        ])
        .output()
        .expect("run backup");
    let manifest = stdout_json(&backup_output);
    assert_eq!(manifest["schema_version"], 27);
    assert_eq!(
        manifest["size_bytes"],
        fs::metadata(&backup).expect("backup").len()
    );
    let manifest_path = PathBuf::from(format!("{}.manifest.json", backup.display()));

    let restored = directory.path().join("restored/agentd.db");
    let restore_output = agentctl()
        .args([
            "cutover",
            "restore",
            "--db-path",
            restored.to_str().expect("restore path"),
            "--backup",
            backup.to_str().expect("backup path"),
            "--manifest",
            manifest_path.to_str().expect("manifest path"),
            "--daemon-address",
            "127.0.0.1:1",
        ])
        .output()
        .expect("run restore");
    let restored_report = stdout_json(&restore_output);
    assert_eq!(
        restored_report["database_sha256"],
        manifest["database_sha256"]
    );
    assert!(restored.is_file());
}

#[test]
fn restore_rejects_bytes_that_do_not_match_the_manifest() {
    let source = source_fixture();
    let directory = tempfile::tempdir().expect("database directory");
    let database = directory.path().join("agentd.db");
    let planned = plan(source.path(), &database);
    let cutover_id = planned["plan"]["id"].as_str().expect("cutover id");
    let backup = directory.path().join("agentd.backup");
    let backup_output = agentctl()
        .args([
            "cutover",
            "backup",
            cutover_id,
            "--db-path",
            database.to_str().expect("database path"),
            "--output",
            backup.to_str().expect("backup path"),
        ])
        .output()
        .expect("run backup");
    stdout_json(&backup_output);
    write(&backup, "tampered");
    let manifest_path = PathBuf::from(format!("{}.manifest.json", backup.display()));

    let output = agentctl()
        .args([
            "cutover",
            "restore",
            "--db-path",
            directory
                .path()
                .join("restore.db")
                .to_str()
                .expect("restore path"),
            "--backup",
            backup.to_str().expect("backup path"),
            "--manifest",
            manifest_path.to_str().expect("manifest path"),
            "--daemon-address",
            "127.0.0.1:1",
        ])
        .output()
        .expect("run restore");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("do not match"));
}

#[test]
fn service_installation_contains_no_legacy_runtime_command() {
    let source = source_fixture();
    let directory = tempfile::tempdir().expect("database directory");
    let database = directory.path().join("agentd.db");
    let planned = plan(source.path(), &database);
    let cutover_id = planned["plan"]["id"].as_str().expect("cutover id");
    let target = directory.path().join("service");
    let output = agentctl()
        .args([
            "cutover",
            "service-install",
            cutover_id,
            "--db-path",
            database.to_str().expect("database path"),
            "--model",
            "team",
            "--target",
            target.to_str().expect("target path"),
            "--agentd-bin",
            env!("CARGO_BIN_EXE_agentctl"),
        ])
        .output()
        .expect("install service");
    stdout_json(&output);
    let unit = fs::read_to_string(target.join("agentd.service")).expect("systemd unit");
    let compose = fs::read_to_string(target.join("compose.yaml")).expect("compose file");
    assert!(unit.contains("AGENTD_NATIVE_RUNTIME=1"));
    assert!(compose.contains("immutable AGENTD_IMAGE digest"));
    assert!(!unit.contains("tmux"));
    assert!(!compose.contains("agent-chat"));
}
