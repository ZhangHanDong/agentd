//! Durable final-cutover operator commands.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agentd_core::ports::{
    BackupManifest, CursorHandoff, CutoverLedgerPort, ServiceInstallation, ServiceModel,
};
use agentd_core::types::{BackupManifestId, CutoverId, ServiceInstallationId};
use agentd_store::{CutoverService, SqliteCutoverLedger, SqliteStore, run_doctor};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use crate::cli::{
    CutoverActivateArgs, CutoverBackupArgs, CutoverCmd, CutoverDoctorArgs, CutoverHandoffArgs,
    CutoverInspectArgs, CutoverMutationArgs, CutoverPlanArgs, CutoverRestoreArgs,
    CutoverRollbackArgs, CutoverServiceInstallArgs, CutoverServiceModel, CutoverSourceStepArgs,
};

const EXIT_INVALID: u8 = 2;
const EXIT_OPERATION: u8 = 3;
const CURRENT_SCHEMA_VERSION: u32 = 27;

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type CommandResult<T> = Result<T, BoxError>;

#[must_use]
pub fn run(cmd: &CutoverCmd) -> ExitCode {
    let result = match cmd {
        CutoverCmd::Plan(args) => plan(args),
        CutoverCmd::Import(args) => source_step(args, SourceStep::Import),
        CutoverCmd::Shadow(args) => source_step(args, SourceStep::Shadow),
        CutoverCmd::Drain(args) => source_step(args, SourceStep::Drain),
        CutoverCmd::Handoff(args) => handoff(args),
        CutoverCmd::Activate(args) => activate(args),
        CutoverCmd::Retire(args) => retire(args),
        CutoverCmd::Inspect(args) => inspect(args),
        CutoverCmd::Rollback(args) => rollback(args),
        CutoverCmd::Doctor(args) => doctor(args),
        CutoverCmd::Backup(args) => backup(args),
        CutoverCmd::Restore(args) => restore(args),
        CutoverCmd::ServiceInstall(args) => service_install(args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(CommandFailure::Invalid(error)) => {
            eprintln!("error: {error}");
            ExitCode::from(EXIT_INVALID)
        }
        Err(CommandFailure::Operation(error)) => {
            eprintln!("error: {error}");
            ExitCode::from(EXIT_OPERATION)
        }
    }
}

#[derive(Debug)]
enum CommandFailure {
    Invalid(String),
    Operation(BoxError),
}

impl From<BoxError> for CommandFailure {
    fn from(error: BoxError) -> Self {
        Self::Operation(error)
    }
}

#[derive(Debug, Clone, Copy)]
enum SourceStep {
    Import,
    Shadow,
    Drain,
}

fn plan(args: &CutoverPlanArgs) -> Result<(), CommandFailure> {
    require_source_root(&args.agent_chat)?;
    let observed_at = now_unix().map_err(CommandFailure::Operation)?;
    let expires_at = observed_at
        .checked_add(i64::try_from(args.rollback_window_seconds).map_err(|_| {
            CommandFailure::Invalid("rollback window exceeds supported range".to_string())
        })?)
        .ok_or_else(|| {
            CommandFailure::Invalid("rollback window overflows time range".to_string())
        })?;
    let target_sha256 = if args.db_path.exists() {
        Some(
            file_sha256(&args.db_path)
                .map_err(CommandFailure::Operation)?
                .0,
        )
    } else {
        None
    };
    let report = run_async(async {
        let store = SqliteStore::connect(&args.db_path).await?;
        Ok(CutoverService::new(store)
            .plan(&args.agent_chat, target_sha256, expires_at, observed_at)
            .await?)
    })?;
    print_json(&report)
}

fn source_step(args: &CutoverSourceStepArgs, step: SourceStep) -> Result<(), CommandFailure> {
    require_source_root(&args.agent_chat)?;
    let cutover_id = parse_cutover_id(&args.cutover_id)?;
    require_idempotency_key(&args.idempotency_key)?;
    let observed_at = now_unix().map_err(CommandFailure::Operation)?;
    let value = run_async(async {
        let store = SqliteStore::connect(&args.db_path).await?;
        let service = CutoverService::new(store);
        let value = match step {
            SourceStep::Import => serde_json::to_value(
                service
                    .import(
                        &cutover_id,
                        &args.agent_chat,
                        &args.idempotency_key,
                        observed_at,
                    )
                    .await?,
            )?,
            SourceStep::Shadow => serde_json::to_value(
                service
                    .shadow(
                        &cutover_id,
                        &args.agent_chat,
                        &args.idempotency_key,
                        observed_at,
                    )
                    .await?,
            )?,
            SourceStep::Drain => serde_json::to_value(
                service
                    .drain(
                        &cutover_id,
                        &args.agent_chat,
                        &args.idempotency_key,
                        observed_at,
                    )
                    .await?,
            )?,
        };
        Ok(value)
    })?;
    print_json(&value)
}

fn handoff(args: &CutoverHandoffArgs) -> Result<(), CommandFailure> {
    let cutover_id = parse_cutover_id(&args.cutover_id)?;
    require_idempotency_key(&args.idempotency_key)?;
    let bytes = fs::read(&args.handoffs_file).map_err(boxed)?;
    let handoffs: Vec<CursorHandoff> = serde_json::from_slice(&bytes)
        .map_err(|error| CommandFailure::Invalid(format!("invalid handoff JSON: {error}")))?;
    let observed_at = now_unix().map_err(CommandFailure::Operation)?;
    let result = run_async(async {
        let store = SqliteStore::connect(&args.db_path).await?;
        Ok(CutoverService::new(store)
            .handoff(&cutover_id, &handoffs, &args.idempotency_key, observed_at)
            .await?)
    })?;
    print_json(&result)
}

fn activate(args: &CutoverActivateArgs) -> Result<(), CommandFailure> {
    require_source_root(&args.agent_chat)?;
    let cutover_id = parse_cutover_id(&args.cutover_id)?;
    require_idempotency_key(&args.idempotency_key)?;
    let observed_at = now_unix().map_err(CommandFailure::Operation)?;
    let result = run_async(async {
        let store = SqliteStore::connect(&args.db_path).await?;
        Ok(CutoverService::new(store)
            .activate(
                &cutover_id,
                &args.agent_chat,
                args.required_project_handoffs,
                &args.idempotency_key,
                observed_at,
            )
            .await?)
    })?;
    print_json(&result)
}

fn retire(args: &CutoverMutationArgs) -> Result<(), CommandFailure> {
    let cutover_id = parse_cutover_id(&args.cutover_id)?;
    require_idempotency_key(&args.idempotency_key)?;
    let observed_at = now_unix().map_err(CommandFailure::Operation)?;
    let result = run_async(async {
        let store = SqliteStore::connect(&args.db_path).await?;
        Ok(CutoverService::new(store)
            .retire(&cutover_id, &args.idempotency_key, observed_at)
            .await?)
    })?;
    print_json(&result)
}

fn inspect(args: &CutoverInspectArgs) -> Result<(), CommandFailure> {
    let cutover_id = parse_cutover_id(&args.cutover_id)?;
    let value = run_async(async {
        let store = SqliteStore::connect(&args.db_path).await?;
        let ledger = SqliteCutoverLedger::new(store.pool().clone());
        let run = ledger
            .load_cutover(&cutover_id)
            .await?
            .ok_or_else(|| agentd_core::ports::CutoverError::NotFound(cutover_id.to_string()))?;
        Ok(json!({
            "run": run,
            "mappings": ledger.mappings(&cutover_id).await?,
            "shadow_decisions": ledger.shadows(&cutover_id).await?,
            "cursor_handoffs": ledger.cursor_handoffs(&cutover_id).await?,
        }))
    })?;
    print_json(&value)
}

fn rollback(args: &CutoverRollbackArgs) -> Result<(), CommandFailure> {
    let cutover_id = parse_cutover_id(&args.cutover_id)?;
    require_idempotency_key(&args.idempotency_key)?;
    if args.reason.trim().is_empty() || args.reason.len() > 512 {
        return Err(CommandFailure::Invalid(
            "rollback reason must contain 1..512 bytes".to_string(),
        ));
    }
    let reason_sha256 = sha256(args.reason.as_bytes());
    let observed_at = now_unix().map_err(CommandFailure::Operation)?;
    let result = run_async(async {
        let store = SqliteStore::connect(&args.db_path).await?;
        Ok(CutoverService::new(store)
            .rollback(
                &cutover_id,
                &reason_sha256,
                &args.idempotency_key,
                observed_at,
            )
            .await?)
    })?;
    print_json(&result)
}

fn doctor(args: &CutoverDoctorArgs) -> Result<(), CommandFailure> {
    let observed_at = now_unix().map_err(CommandFailure::Operation)?;
    let report = run_async(async {
        let store = SqliteStore::connect(&args.db_path).await?;
        Ok(run_doctor(store.pool(), observed_at).await?)
    })?;
    let ok = report.ok;
    print_json(&report)?;
    if ok {
        Ok(())
    } else {
        Err(CommandFailure::Operation(boxed_message(
            "doctor found failing control-plane checks",
        )))
    }
}

fn backup(args: &CutoverBackupArgs) -> Result<(), CommandFailure> {
    let cutover_id = parse_cutover_id(&args.cutover_id)?;
    if args.output.exists() {
        return Err(CommandFailure::Invalid(format!(
            "backup target already exists: {}",
            args.output.display()
        )));
    }
    let output = absolute_new_path(&args.output)?;
    let output_parent = output.parent().ok_or_else(|| {
        CommandFailure::Invalid("backup target must have a parent directory".to_string())
    })?;
    fs::create_dir_all(output_parent).map_err(boxed)?;
    let manifest_path = backup_manifest_path(&output);
    if manifest_path.exists() {
        return Err(CommandFailure::Invalid(format!(
            "backup manifest target already exists: {}",
            manifest_path.display()
        )));
    }
    let created_at = now_unix().map_err(CommandFailure::Operation)?;
    let manifest = run_async(async {
        let store = SqliteStore::connect(&args.db_path).await?;
        let schema_version = schema_version(store.pool()).await?;
        if schema_version != CURRENT_SCHEMA_VERSION {
            return Err(boxed_message(format!(
                "backup requires schema {CURRENT_SCHEMA_VERSION}, found {schema_version}"
            )));
        }
        sqlx::query("VACUUM INTO ?")
            .bind(output.to_string_lossy().as_ref())
            .execute(store.pool())
            .await?;
        let (database_sha256, size_bytes) = file_sha256(&output)?;
        let manifest = BackupManifest {
            id: BackupManifestId::new(),
            cutover_id,
            database_sha256,
            schema_version,
            size_bytes,
            storage_ref: output.to_string_lossy().into_owned(),
            created_at,
        };
        atomic_write_json(&manifest_path, &manifest)?;
        let ledger = SqliteCutoverLedger::new(store.pool().clone());
        Ok(ledger.record_backup(&manifest).await?)
    })?;
    print_json(&manifest)
}

fn restore(args: &CutoverRestoreArgs) -> Result<(), CommandFailure> {
    refuse_running_daemon(&args.daemon_address)?;
    if !args.backup.is_file() || !args.manifest.is_file() {
        return Err(CommandFailure::Invalid(
            "backup and manifest must both be regular files".to_string(),
        ));
    }
    let manifest: BackupManifest =
        serde_json::from_slice(&fs::read(&args.manifest).map_err(boxed)?).map_err(|error| {
            CommandFailure::Invalid(format!("invalid backup manifest: {error}"))
        })?;
    let (digest, size_bytes) = file_sha256(&args.backup).map_err(CommandFailure::Operation)?;
    if digest != manifest.database_sha256
        || size_bytes != manifest.size_bytes
        || manifest.schema_version != CURRENT_SCHEMA_VERSION
    {
        return Err(CommandFailure::Invalid(
            "backup bytes do not match the declared manifest".to_string(),
        ));
    }
    let observed_schema = run_async(read_only_schema(&args.backup))?;
    if observed_schema != manifest.schema_version {
        return Err(CommandFailure::Invalid(format!(
            "backup schema mismatch: manifest={}, database={observed_schema}",
            manifest.schema_version
        )));
    }
    atomic_restore(&args.backup, &args.db_path).map_err(CommandFailure::Operation)?;
    print_json(&json!({
        "restored": args.db_path,
        "backup_manifest_id": manifest.id,
        "database_sha256": manifest.database_sha256,
        "schema_version": manifest.schema_version,
    }))
}

fn service_install(args: &CutoverServiceInstallArgs) -> Result<(), CommandFailure> {
    let cutover_id = parse_cutover_id(&args.cutover_id)?;
    let agentd_bin = fs::canonicalize(&args.agentd_bin).map_err(|error| {
        CommandFailure::Invalid(format!("agentd binary is unavailable: {error}"))
    })?;
    if !agentd_bin.is_file() {
        return Err(CommandFailure::Invalid(
            "agentd binary must be a regular file".to_string(),
        ));
    }
    let db_path = absolute_new_path(&args.db_path)?;
    let target = absolute_new_path(&args.target)?;
    fs::create_dir_all(&target).map_err(boxed)?;
    let model = service_model(args.model);
    let assets = service_assets(model, &agentd_bin, &db_path, args.port);
    let mut entries = Vec::with_capacity(assets.len());
    for (relative, bytes) in assets {
        let path = target.join(relative);
        atomic_write(&path, bytes.as_bytes()).map_err(CommandFailure::Operation)?;
        entries.push(json!({
            "path": path,
            "sha256": sha256(bytes.as_bytes()),
        }));
    }
    let canonical_manifest = serde_json::to_vec(&json!({
        "model": model,
        "assets": entries,
    }))
    .map_err(boxed)?;
    let installation = ServiceInstallation {
        id: ServiceInstallationId::new(),
        cutover_id,
        model,
        manifest_sha256: sha256(&canonical_manifest),
        target_ref_sha256: sha256(target.to_string_lossy().as_bytes()),
        installed_at: now_unix().map_err(CommandFailure::Operation)?,
    };
    let installation = run_async(async {
        let store = SqliteStore::connect(&args.db_path).await?;
        let ledger = SqliteCutoverLedger::new(store.pool().clone());
        Ok(ledger.record_service_installation(&installation).await?)
    })?;
    print_json(&installation)
}

async fn schema_version(pool: &sqlx::SqlitePool) -> CommandResult<u32> {
    let value =
        sqlx::query_scalar::<_, String>("SELECT value FROM schema_meta WHERE key = 'version'")
            .fetch_one(pool)
            .await?;
    Ok(value.parse()?)
}

async fn read_only_schema(path: &Path) -> CommandResult<u32> {
    let options = SqliteConnectOptions::new().filename(path).read_only(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    let version = schema_version(&pool).await?;
    pool.close().await;
    Ok(version)
}

fn service_model(model: CutoverServiceModel) -> ServiceModel {
    match model {
        CutoverServiceModel::Local => ServiceModel::Local,
        CutoverServiceModel::Team => ServiceModel::Team,
        CutoverServiceModel::Fleet => ServiceModel::Fleet,
    }
}

fn service_assets(
    model: ServiceModel,
    agentd_bin: &Path,
    db_path: &Path,
    port: u16,
) -> Vec<(&'static str, String)> {
    let bin = agentd_bin.to_string_lossy();
    let db = db_path.to_string_lossy();
    match model {
        ServiceModel::Local => vec![(
            "io.agentd.plist",
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>io.agentd</string>\n<key>ProgramArguments</key><array><string>{bin}</string><string>--db-path</string><string>{db}</string><string>--port</string><string>{port}</string></array>\n<key>EnvironmentVariables</key><dict><key>AGENTD_NATIVE_RUNTIME</key><string>1</string></dict>\n<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>\n</dict></plist>\n"
            ),
        )],
        ServiceModel::Team => vec![
            (
                "agentd.service",
                format!(
                    "[Unit]\nDescription=agentd native control plane\nAfter=network-online.target\n\n[Service]\nType=simple\nExecStart={bin} --db-path {db} --port {port}\nEnvironment=AGENTD_NATIVE_RUNTIME=1\nRestart=on-failure\nNoNewPrivileges=true\n\n[Install]\nWantedBy=multi-user.target\n"
                ),
            ),
            (
                "compose.yaml",
                format!(
                    "services:\n  agentd:\n    image: ${{AGENTD_IMAGE:?set immutable AGENTD_IMAGE digest}}\n    command: [\"agentd\", \"--db-path\", \"/var/lib/agentd/agentd.db\", \"--port\", \"{port}\"]\n    environment:\n      AGENTD_NATIVE_RUNTIME: \"1\"\n    volumes:\n      - {db}:/var/lib/agentd/agentd.db\n    restart: unless-stopped\n"
                ),
            ),
        ],
        ServiceModel::Fleet => vec![(
            "agentd-fleet.env",
            format!(
                "AGENTD_NATIVE_RUNTIME=1\nAGENTD_DB_PATH={db}\nAGENTD_PORT={port}\nAGENTD_WORKER_DISPATCH=pull\nAGENTD_IMAGE_POLICY=immutable-digest\n"
            ),
        )],
    }
}

fn atomic_restore(backup: &Path, target: &Path) -> CommandResult<()> {
    let parent = target
        .parent()
        .ok_or_else(|| boxed_message("restore target must have a parent directory"))?;
    fs::create_dir_all(parent)?;
    let temp = sibling_temp_path(target, "restore");
    if temp.exists() {
        return Err(boxed_message(format!(
            "restore staging path already exists: {}",
            temp.display()
        )));
    }
    fs::copy(backup, &temp)?;
    OpenOptions::new().write(true).open(&temp)?.sync_all()?;
    let (source_digest, source_size) = file_sha256(backup)?;
    let (staged_digest, staged_size) = file_sha256(&temp)?;
    if source_digest != staged_digest || source_size != staged_size {
        fs::remove_file(&temp)?;
        return Err(boxed_message("restore staging digest mismatch"));
    }

    let displaced = sibling_temp_path(target, "pre-restore");
    if displaced.exists() {
        fs::remove_file(&temp)?;
        return Err(boxed_message(format!(
            "pre-restore path already exists: {}",
            displaced.display()
        )));
    }
    if target.exists() {
        fs::rename(target, &displaced)?;
    }
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", target.to_string_lossy()));
        if sidecar.exists() {
            fs::rename(
                &sidecar,
                PathBuf::from(format!("{}{suffix}", displaced.to_string_lossy())),
            )?;
        }
    }
    if let Err(error) = fs::rename(&temp, target) {
        if displaced.exists() {
            let _ = fs::rename(&displaced, target);
        }
        return Err(Box::new(error));
    }
    OpenOptions::new().read(true).open(parent)?.sync_all()?;
    Ok(())
}

fn refuse_running_daemon(address: &str) -> Result<(), CommandFailure> {
    let addresses = address.to_socket_addrs().map_err(|error| {
        CommandFailure::Invalid(format!("invalid daemon address {address}: {error}"))
    })?;
    for address in addresses {
        if TcpStream::connect_timeout(&address, Duration::from_millis(250)).is_ok() {
            return Err(CommandFailure::Invalid(format!(
                "refusing restore while a service is listening on {address}"
            )));
        }
    }
    Ok(())
}

fn parse_cutover_id(value: &str) -> Result<CutoverId, CommandFailure> {
    if !value.starts_with("co_") || value.len() > 64 || value.len() < 4 {
        return Err(CommandFailure::Invalid(
            "cutover id must be a bounded co_ identifier".to_string(),
        ));
    }
    Ok(CutoverId::from_string(value))
}

fn require_idempotency_key(value: &str) -> Result<(), CommandFailure> {
    if value.trim().is_empty() || value.len() > 128 {
        return Err(CommandFailure::Invalid(
            "idempotency key must contain 1..128 bytes".to_string(),
        ));
    }
    Ok(())
}

fn require_source_root(path: &Path) -> Result<(), CommandFailure> {
    if !path.is_dir() {
        return Err(CommandFailure::Invalid(format!(
            "agent-chat source is not a directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn absolute_new_path(path: &Path) -> Result<PathBuf, CommandFailure> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir().map_err(boxed)?.join(path))
}

fn file_sha256(path: &Path) -> CommandResult<(String, u64)> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut size = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        size = size
            .checked_add(u64::try_from(read)?)
            .ok_or_else(|| boxed_message("file size overflow"))?;
    }
    Ok((hex::encode(digest.finalize()), size))
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn backup_manifest_path(output: &Path) -> PathBuf {
    PathBuf::from(format!("{}.manifest.json", output.to_string_lossy()))
}

fn atomic_write_json(path: &Path, value: &impl Serialize) -> CommandResult<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> CommandResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| boxed_message("target path must have a parent directory"))?;
    fs::create_dir_all(parent)?;
    let temp = sibling_temp_path(path, "write");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temp, path)?;
    OpenOptions::new().read(true).open(parent)?.sync_all()?;
    Ok(())
}

fn sibling_temp_path(path: &Path, operation: &str) -> PathBuf {
    PathBuf::from(format!(
        "{}.agentd-{operation}-{}",
        path.to_string_lossy(),
        std::process::id()
    ))
}

fn now_unix() -> CommandResult<i64> {
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    Ok(i64::try_from(seconds)?)
}

fn print_json(value: &impl Serialize) -> Result<(), CommandFailure> {
    println!("{}", serde_json::to_string_pretty(value).map_err(boxed)?);
    Ok(())
}

fn run_async<T, F>(future: F) -> Result<T, CommandFailure>
where
    F: std::future::Future<Output = CommandResult<T>>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(boxed)?;
    runtime.block_on(future).map_err(CommandFailure::Operation)
}

fn boxed(error: impl std::error::Error + Send + Sync + 'static) -> BoxError {
    Box::new(error)
}

fn boxed_message(message: impl Into<String>) -> BoxError {
    Box::new(std::io::Error::other(message.into()))
}
