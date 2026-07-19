//! `agentctl parity` — read-only replacement-audit helpers for the agent-chat
//! cutover work. P200 intentionally does not implement missing capabilities; it
//! turns the current gap list into a repeatable gate.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use agentd_store::SqliteStore;
use agentd_store::agent_chat_import::{
    self, AgentChatAgentImportReport, AgentChatImportMode, AgentChatImportOptions,
    AgentChatMessageImportOptions, AgentChatMessageImportReport, AgentChatTaskImportOptions,
    AgentChatTaskImportReport,
};

use crate::cli::{
    ParityAgentImportArgs, ParityAgentShadowArgs, ParityAuditArgs, ParityCmd,
    ParityMessageImportArgs, ParityMessageShadowArgs, ParityMigrationArgs, ParityRollbackArgs,
    ParityTaskImportArgs, ParityTaskShadowArgs,
};

const EXIT_GAPS: u8 = 1;
const EXIT_INVALID: u8 = 2;
const EXIT_IMPORT: u8 = 3;
const REQUIRED_CATEGORIES: &[&str] = &[
    "registry",
    "messaging",
    "task_graph",
    "scheduler",
    "runtime_launch",
    "dashboard_cli",
    "matrix_remote",
    "migration_cutover",
    "auth",
    "real_execution",
];
const ALLOWED_STATUS: &[&str] = &["covered", "partial", "missing", "deferred", "external"];
const OK_REQUIRED_STATUS: &[&str] = &["covered", "deferred", "external"];

#[derive(Debug, Clone)]
struct ParityRow {
    capability: String,
    category: String,
    priority: String,
    source: String,
    status: String,
    decision: String,
    phase: String,
}

#[derive(Debug)]
struct AuditSummary {
    required_total: usize,
    covered: usize,
    partial: usize,
    missing: usize,
    deferred: usize,
    external: usize,
    gaps: Vec<ParityRow>,
}

#[must_use]
pub fn run(cmd: &ParityCmd) -> ExitCode {
    match cmd {
        ParityCmd::Audit(args) => audit(args),
        ParityCmd::ImportAgents(args) => import_agents(args),
        ParityCmd::ShadowAgents(args) => shadow_agents(args),
        ParityCmd::ImportMessages(args) => import_messages(args),
        ParityCmd::ShadowMessages(args) => shadow_messages(args),
        ParityCmd::ImportTasks(args) => import_tasks(args),
        ParityCmd::ShadowTasks(args) => shadow_tasks(args),
        ParityCmd::MigrateSupportedState(args) => migrate_supported_state(args),
        ParityCmd::RollbackCutover(args) => rollback_cutover(args),
    }
}

fn rollback_cutover(args: &ParityRollbackArgs) -> ExitCode {
    if !args.execute {
        println!(
            "dry-run: would rollback project={} with lease_epoch={}",
            args.project_id, args.lease_epoch
        );
        return ExitCode::SUCCESS;
    }
    match run_async(async {
        let store = SqliteStore::connect(&args.db_path).await?;
        agentd_store::cutover_repo::rollback(store.pool(), &args.project_id, args.lease_epoch).await
    }) {
        Ok(state) => {
            println!(
                "cutover rollback project={} phase={:?} lease_epoch={}",
                state.project_id, state.phase, state.lease_epoch
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: cutover rollback failed: {error}");
            ExitCode::from(EXIT_IMPORT)
        }
    }
}

fn migrate_supported_state(args: &ParityMigrationArgs) -> ExitCode {
    if let Err(err) = validate_agent_chat_path(&args.agent_chat) {
        eprintln!("error: invalid agent-chat path: {err}");
        return ExitCode::from(EXIT_INVALID);
    }
    let mode = if args.execute {
        AgentChatImportMode::Execute
    } else {
        AgentChatImportMode::DryRun
    };
    let binding = match (
        &args.project_id,
        &args.authority_revision,
        args.matrix_cursor,
        args.lease_epoch,
    ) {
        (None, None, None, None) => None,
        (Some(project_id), Some(authority_revision), Some(matrix_cursor), Some(lease_epoch)) => {
            Some((
                project_id.clone(),
                authority_revision.clone(),
                matrix_cursor,
                lease_epoch,
            ))
        }
        _ => {
            eprintln!(
                "error: --project-id, --authority-revision, --matrix-cursor, and --lease-epoch must be supplied together"
            );
            return ExitCode::from(EXIT_INVALID);
        }
    };
    if binding.is_some() && !args.execute {
        eprintln!("error: project binding requires --execute");
        return ExitCode::from(EXIT_INVALID);
    }
    let result = run_async(async {
        let store = SqliteStore::connect(&args.db_path).await?;
        let report =
            agent_chat_import::migrate_supported_state(store.pool(), &args.agent_chat, mode)
                .await?;
        if let Some((project_id, authority_revision, matrix_cursor, lease_epoch)) = binding {
            if !report.ok {
                return Err(agentd_store::StoreError::Invariant(
                    "supported-state migration report is not healthy".into(),
                ));
            }
            let state = agentd_store::cutover_repo::transition(
                store.pool(),
                &project_id,
                agentd_store::cutover_repo::CutoverPhase::Observe,
                &authority_revision,
                matrix_cursor,
                lease_epoch,
            )
            .await?;
            Ok((report, Some(state)))
        } else {
            Ok((report, None))
        }
    });
    match result {
        Ok((report, cutover_state)) => {
            println!("agent-chat supported-state migration ({})", report.mode);
            println!("agents imported={}", report.agents.agents.imported);
            println!("messages imported={}", report.messages.messages.imported);
            println!("cursors imported={}", report.messages.cursors.imported);
            println!("tasks imported={}", report.tasks.tasks.imported);
            println!("task_graphs imported={}", report.tasks.task_graphs.imported);
            if let Some(state) = cutover_state {
                println!(
                    "cutover project={} phase={:?}",
                    state.project_id, state.phase
                );
            }
            if report.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(EXIT_IMPORT)
            }
        }
        Err(err) => {
            eprintln!("error: supported-state migration failed: {err}");
            ExitCode::from(EXIT_IMPORT)
        }
    }
}

fn audit(args: &ParityAuditArgs) -> ExitCode {
    if let Err(err) = validate_agent_chat_path(&args.agent_chat) {
        eprintln!("error: invalid agent-chat path: {err}");
        return ExitCode::from(EXIT_INVALID);
    }

    let map_path = resolve_map_path(&args.map);
    let markdown = match std::fs::read_to_string(&map_path) {
        Ok(markdown) => markdown,
        Err(err) => {
            eprintln!(
                "error: cannot read parity map {}: {err}",
                map_path.display()
            );
            return ExitCode::from(EXIT_INVALID);
        }
    };
    let rows = match parse_map(&markdown) {
        Ok(rows) => rows,
        Err(err) => {
            eprintln!("error: invalid parity map: {err}");
            return ExitCode::from(EXIT_INVALID);
        }
    };
    if let Err(err) = validate_rows(&rows, &args.agent_chat) {
        eprintln!("error: invalid parity map: {err}");
        return ExitCode::from(EXIT_INVALID);
    }

    let summary = summarize(&rows);
    print_summary(&args.agent_chat, &map_path, &summary);
    if summary.gaps.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_GAPS)
    }
}

fn import_agents(args: &ParityAgentImportArgs) -> ExitCode {
    if let Err(err) = validate_agent_chat_path(&args.agent_chat) {
        eprintln!("error: invalid agent-chat path: {err}");
        return ExitCode::from(EXIT_INVALID);
    }

    let result = if args.execute {
        run_async(async {
            let store = SqliteStore::connect(&args.db_path).await?;
            agent_chat_import::import_agents_from_agent_chat(
                store.pool(),
                &args.agent_chat,
                AgentChatImportOptions {
                    mode: AgentChatImportMode::Execute,
                },
            )
            .await
        })
    } else {
        agent_chat_import::plan_agents_from_agent_chat(&args.agent_chat).map_err(|err| {
            let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(err);
            boxed
        })
    };

    match result {
        Ok(report) => {
            print_agent_import_report(&report);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(EXIT_IMPORT)
        }
    }
}

fn shadow_agents(args: &ParityAgentShadowArgs) -> ExitCode {
    if let Err(err) = validate_agent_chat_path(&args.agent_chat) {
        eprintln!("error: invalid agent-chat path: {err}");
        return ExitCode::from(EXIT_INVALID);
    }

    match run_async(async {
        let store = SqliteStore::connect(&args.db_path).await?;
        agent_chat_import::shadow_agents(store.pool(), &args.agent_chat).await
    }) {
        Ok(report) => {
            let ok = report.ok;
            print_agent_import_report(&report);
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(EXIT_GAPS)
            }
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(EXIT_IMPORT)
        }
    }
}

fn import_messages(args: &ParityMessageImportArgs) -> ExitCode {
    if let Err(err) = validate_agent_chat_path(&args.agent_chat) {
        eprintln!("error: invalid agent-chat path: {err}");
        return ExitCode::from(EXIT_INVALID);
    }

    let result = if args.execute {
        run_async(async {
            let store = SqliteStore::connect(&args.db_path).await?;
            agent_chat_import::import_messages_from_agent_chat(
                store.pool(),
                &args.agent_chat,
                AgentChatMessageImportOptions {
                    mode: AgentChatImportMode::Execute,
                },
            )
            .await
        })
    } else {
        agent_chat_import::plan_messages_from_agent_chat(&args.agent_chat).map_err(|err| {
            let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(err);
            boxed
        })
    };

    match result {
        Ok(report) => {
            print_message_import_report(&report);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(EXIT_IMPORT)
        }
    }
}

fn shadow_messages(args: &ParityMessageShadowArgs) -> ExitCode {
    if let Err(err) = validate_agent_chat_path(&args.agent_chat) {
        eprintln!("error: invalid agent-chat path: {err}");
        return ExitCode::from(EXIT_INVALID);
    }

    match run_async(async {
        let store = SqliteStore::connect(&args.db_path).await?;
        agent_chat_import::shadow_messages(store.pool(), &args.agent_chat).await
    }) {
        Ok(report) => {
            let ok = report.ok;
            print_message_import_report(&report);
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(EXIT_GAPS)
            }
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(EXIT_IMPORT)
        }
    }
}

fn import_tasks(args: &ParityTaskImportArgs) -> ExitCode {
    if let Err(err) = validate_agent_chat_path(&args.agent_chat) {
        eprintln!("error: invalid agent-chat path: {err}");
        return ExitCode::from(EXIT_INVALID);
    }

    let result = if args.execute {
        run_async(async {
            let store = SqliteStore::connect(&args.db_path).await?;
            agent_chat_import::import_tasks_from_agent_chat(
                store.pool(),
                &args.agent_chat,
                AgentChatTaskImportOptions {
                    mode: AgentChatImportMode::Execute,
                },
            )
            .await
        })
    } else {
        agent_chat_import::plan_tasks_from_agent_chat(&args.agent_chat).map_err(|err| {
            let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(err);
            boxed
        })
    };

    match result {
        Ok(report) => {
            print_task_import_report(&report);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(EXIT_IMPORT)
        }
    }
}

fn shadow_tasks(args: &ParityTaskShadowArgs) -> ExitCode {
    if let Err(err) = validate_agent_chat_path(&args.agent_chat) {
        eprintln!("error: invalid agent-chat path: {err}");
        return ExitCode::from(EXIT_INVALID);
    }

    match run_async(async {
        let store = SqliteStore::connect(&args.db_path).await?;
        agent_chat_import::shadow_tasks(store.pool(), &args.agent_chat).await
    }) {
        Ok(report) => {
            let ok = report.ok;
            print_task_import_report(&report);
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(EXIT_GAPS)
            }
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(EXIT_IMPORT)
        }
    }
}

fn validate_agent_chat_path(path: &Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    for file in ["backend-v2.js", "lib/mcp-server-core.js", "server.js"] {
        let expected = path.join(file);
        if !expected.is_file() {
            return Err(format!("{} is missing", expected.display()));
        }
    }
    Ok(())
}

fn resolve_map_path(path: &Path) -> PathBuf {
    if path.is_absolute() || path.exists() {
        return path.to_path_buf();
    }
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let from_repo = repo_root.join(path);
    if from_repo.exists() {
        from_repo
    } else {
        path.to_path_buf()
    }
}

fn parse_map(markdown: &str) -> Result<Vec<ParityRow>, String> {
    let mut rows = Vec::new();
    let mut table_started = false;
    for line in markdown
        .lines()
        .filter(|line| line.trim_start().starts_with('|'))
    {
        if line.contains("---") {
            continue;
        }
        if !table_started {
            table_started = true;
            continue;
        }
        let cells = line
            .trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect::<Vec<_>>();
        if cells.len() != 7 {
            return Err(format!("expected 7 columns in row: {line}"));
        }
        rows.push(ParityRow {
            capability: cells[0].to_string(),
            category: cells[1].to_string(),
            priority: cells[2].to_string(),
            source: cells[3].to_string(),
            status: cells[4].to_string(),
            decision: cells[5].to_string(),
            phase: cells[6].to_string(),
        });
    }
    if rows.is_empty() {
        return Err("no capability rows found".to_string());
    }
    Ok(rows)
}

fn validate_rows(rows: &[ParityRow], agent_chat: &Path) -> Result<(), String> {
    let mut categories = REQUIRED_CATEGORIES
        .iter()
        .copied()
        .map(|category| (category, false))
        .collect::<std::collections::BTreeMap<_, _>>();
    let allowed = ALLOWED_STATUS
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();

    for row in rows.iter().filter(|row| row.priority == "required") {
        if !allowed.contains(row.status.as_str()) {
            return Err(format!(
                "{} has unsupported required status {}",
                row.capability, row.status
            ));
        }
        if row.status == "unknown" {
            return Err(format!("{} has forbidden unknown status", row.capability));
        }
        if row.decision.trim().is_empty() {
            return Err(format!("{} has no replacement decision", row.capability));
        }
        if !row.source.starts_with(&agent_chat.display().to_string()) {
            return Err(format!(
                "{} source is outside agent-chat path: {}",
                row.capability, row.source
            ));
        }
        if let Some(seen) = categories.get_mut(row.category.as_str()) {
            *seen = true;
        }
    }

    let missing = categories
        .into_iter()
        .filter_map(|(category, seen)| (!seen).then_some(category))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "missing required categories: {}",
            missing.join(", ")
        ));
    }
    Ok(())
}

fn summarize(rows: &[ParityRow]) -> AuditSummary {
    let mut summary = AuditSummary {
        required_total: 0,
        covered: 0,
        partial: 0,
        missing: 0,
        deferred: 0,
        external: 0,
        gaps: Vec::new(),
    };
    for row in rows.iter().filter(|row| row.priority == "required") {
        summary.required_total += 1;
        match row.status.as_str() {
            "covered" => summary.covered += 1,
            "partial" => summary.partial += 1,
            "missing" => summary.missing += 1,
            "deferred" => summary.deferred += 1,
            "external" => summary.external += 1,
            _ => {}
        }
        if !OK_REQUIRED_STATUS.contains(&row.status.as_str()) {
            summary.gaps.push(row.clone());
        }
    }
    summary
}

fn print_summary(agent_chat: &Path, map: &Path, summary: &AuditSummary) {
    println!("agent-chat parity audit");
    println!("agent_chat={}", agent_chat.display());
    println!("map={}", map.display());
    println!(
        "required_total={} covered={} partial={} missing={} deferred={} external={}",
        summary.required_total,
        summary.covered,
        summary.partial,
        summary.missing,
        summary.deferred,
        summary.external
    );
    if summary.gaps.is_empty() {
        println!("required gaps: none");
    } else {
        println!("required gaps:");
        for row in &summary.gaps {
            println!(
                "- {} [{}] {}: {}",
                row.capability, row.status, row.phase, row.decision
            );
        }
    }
}

fn print_agent_import_report(report: &AgentChatAgentImportReport) {
    println!("agent-chat agent import");
    println!("mode={}", report.mode);
    println!("ok={}", report.ok);
    println!(
        "agents source={} planned={} imported={} skipped={} missing={}",
        report.agents.source,
        report.agents.planned,
        report.agents.imported,
        report.agents.skipped,
        report.agents.missing
    );
    if report.warnings.is_empty() {
        println!("warnings: none");
    } else {
        println!("warnings:");
        for warning in &report.warnings {
            println!("- {warning}");
        }
    }
    if report.drift.is_empty() {
        println!("drift: none");
    } else {
        println!("drift:");
        for drift in &report.drift {
            println!("- {drift}");
        }
    }
}

fn print_message_import_report(report: &AgentChatMessageImportReport) {
    println!("agent-chat message import");
    println!("mode={}", report.mode);
    println!("ok={}", report.ok);
    println!(
        "messages source={} planned={} imported={} direct={} group={} skipped={} missing={}",
        report.messages.source,
        report.messages.planned,
        report.messages.imported,
        report.messages.direct,
        report.messages.group,
        report.messages.skipped,
        report.messages.missing
    );
    println!(
        "groups source={} planned={} imported={} skipped={} missing={}",
        report.groups.source,
        report.groups.planned,
        report.groups.imported,
        report.groups.skipped,
        report.groups.missing
    );
    println!(
        "cursors source={} planned={} imported={} skipped={} missing={}",
        report.cursors.source,
        report.cursors.planned,
        report.cursors.imported,
        report.cursors.skipped,
        report.cursors.missing
    );
    if report.warnings.is_empty() {
        println!("warnings: none");
    } else {
        println!("warnings:");
        for warning in &report.warnings {
            println!("- {warning}");
        }
    }
    if report.drift.is_empty() {
        println!("drift: none");
    } else {
        println!("drift:");
        for drift in &report.drift {
            println!("- {drift}");
        }
    }
}

fn print_task_import_report(report: &AgentChatTaskImportReport) {
    println!("agent-chat task import");
    println!("mode={}", report.mode);
    println!("ok={}", report.ok);
    println!(
        "tasks source={} planned={} imported={} skipped={} missing={}",
        report.tasks.source,
        report.tasks.planned,
        report.tasks.imported,
        report.tasks.skipped,
        report.tasks.missing
    );
    println!(
        "task_graphs source={} planned={} imported={} skipped={} missing={}",
        report.task_graphs.source,
        report.task_graphs.planned,
        report.task_graphs.imported,
        report.task_graphs.skipped,
        report.task_graphs.missing
    );
    if report.warnings.is_empty() {
        println!("warnings: none");
    } else {
        println!("warnings:");
        for warning in &report.warnings {
            println!("- {warning}");
        }
    }
    if report.drift.is_empty() {
        println!("drift: none");
    } else {
        println!("drift:");
        for drift in &report.drift {
            println!("- {drift}");
        }
    }
}

fn run_async<T, F>(future: F) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
where
    F: std::future::Future<Output = Result<T, agentd_store::StoreError>>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(future).map_err(|err| {
        let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(err);
        boxed
    })
}
