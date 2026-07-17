//! The `parallel.fan_out` handler (D1 park, D7 no disk bundle). `run` computes
//! an in-memory `context_sha` over the serialized context + node id (so every
//! reviewer pins one snapshot), records a review-run row, spawns N reviewer
//! agents, stages the review-run id (for the paired `fan_in`), and parks.
//! `resume` records each verdict and re-parks until all N have arrived.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::CoreError;
use crate::engine::{EngineEvent, HandlerStep, ParkReason};
use crate::handler::{
    Handler, HandlerCtx, append_agent_allocation_prompt_context, sha256_hex, spawn_request,
    stage_agent_allocation,
};
use crate::ports::{AgentAllocation, AgentAllocationRequest, AgentAllocationStatus, DrawerHit};
use crate::types::{AgentId, NodeId, Outcome, ReviewRunId, ReviewVerdict};

#[derive(Debug)]
pub struct FanOutHandler;

/// The reviewer roles declared by the node's `reviewers` comma-list.
fn reviewer_roles(ctx: &HandlerCtx<'_>) -> Vec<String> {
    ctx.node_attr("reviewers")
        .map(|s| {
            s.split(',')
                .map(|r| r.trim().to_string())
                .filter(|r| !r.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

#[async_trait::async_trait]
impl Handler for FanOutHandler {
    async fn run(&self, ctx: &mut HandlerCtx<'_>) -> Result<HandlerStep, CoreError> {
        let roles = reviewer_roles(ctx);
        if roles.is_empty() {
            return Err(CoreError::Invariant(format!(
                "fan_out node '{}' declares no reviewers",
                ctx.node.id
            )));
        }
        let expected = roles.len();
        let run_id = ctx.run_id.clone();
        let node_id = NodeId::parsed(&ctx.node.id);

        // Deterministic context_sha: serialized context bytes + the node id.
        let mut to_hash = serde_json::to_vec(ctx.context)?;
        to_hash.extend_from_slice(node_id.as_str().as_bytes());
        let context_sha = sha256_hex(&to_hash);

        let round = delphi_round(ctx);
        let stance_queries = reviewer_map(ctx, "stance_queries", &roles)?;
        ensure_distinct_stance_queries(ctx, stance_queries.as_ref())?;
        let prompt_profiles = reviewer_map(ctx, "prompt_profiles", &roles)?;
        let stance_packs = stance_packs(ctx, &roles, stance_queries.as_ref()).await;

        let review_run_id = ctx
            .ports
            .store
            .insert_review_run(&run_id, &node_id, expected, round, &context_sha)
            .await?;

        let reviewer_worktrees =
            reviewer_worktrees(ctx, &review_run_id, &roles, round, &context_sha).await?;
        persist_reviewer_worktrees(ctx, &review_run_id, &reviewer_worktrees).await?;

        for reviewer in &reviewer_worktrees {
            let prompt = review_prompt(&ReviewPromptInput {
                ctx,
                review_run_id: &review_run_id,
                reviewer_id: &reviewer.agent_id,
                allocation: &reviewer.allocation,
                round,
                context_sha: &context_sha,
                stance_query: stance_queries
                    .as_ref()
                    .and_then(|queries| queries.get(&reviewer.role))
                    .map(String::as_str),
                prompt_profile: prompt_profiles
                    .as_ref()
                    .and_then(|profiles| profiles.get(&reviewer.role))
                    .map(String::as_str),
                stance_hits: stance_packs
                    .get(&reviewer.role)
                    .map_or(&[][..], Vec::as_slice),
                review_worktree: &reviewer.worktree,
            });
            if reviewer.allocation.status == AgentAllocationStatus::Queued {
                let base_prompt = queued_review_base_prompt(
                    ctx,
                    round,
                    &context_sha,
                    prompt_profiles
                        .as_ref()
                        .and_then(|profiles| profiles.get(&reviewer.role))
                        .map(String::as_str),
                    stance_queries
                        .as_ref()
                        .and_then(|queries| queries.get(&reviewer.role))
                        .map(String::as_str),
                    stance_packs
                        .get(&reviewer.role)
                        .map_or(&[][..], Vec::as_slice),
                );
                stage_agent_allocation(ctx, &reviewer.allocation);
                stage_queued_fan_out_dispatch(
                    ctx,
                    &review_run_id,
                    &reviewer.role,
                    round,
                    &context_sha,
                    &base_prompt,
                    &reviewer.worktree,
                );
                continue;
            }
            let request = spawn_request(
                reviewer.agent_id.as_str(),
                None,
                Some(prompt),
                &reviewer.worktree,
            );
            ctx.ports
                .backend
                .dispatch_allocated(request, &reviewer.allocation)
                .await?;
            stage_agent_allocation(ctx, &reviewer.allocation);
        }

        stage_review_run_context(ctx, &review_run_id, &context_sha, round);
        Ok(HandlerStep::Park(ParkReason::ReviewVerdicts {
            review_run_id,
            expected,
            round,
        }))
    }

    async fn resume(
        &self,
        ctx: &mut HandlerCtx<'_>,
        event: EngineEvent,
    ) -> Result<HandlerStep, CoreError> {
        let EngineEvent::ReviewVerdictSubmitted {
            review_run_id,
            reviewer_id,
            verdict,
            findings,
        } = event
        else {
            return Err(CoreError::Invariant(
                "fan_out resumed with a non-ReviewVerdictSubmitted event".to_string(),
            ));
        };
        ctx.ports
            .store
            .insert_review_verdict(
                &review_run_id,
                ReviewVerdict {
                    reviewer_id: reviewer_id.clone(),
                    value: verdict,
                    findings,
                },
            )
            .await?;
        if let Some(drained) = ctx.ports.agent_allocator.release(&reviewer_id).await? {
            stage_agent_allocation(ctx, &drained);
        }
        release_reviewer_worktree(ctx, &review_run_id, &reviewer_id).await?;
        let collected = ctx.ports.store.count_verdicts(&review_run_id).await?;
        // Authoritative `expected` is the value stored at run() time, NOT a
        // re-derivation from the live node attr — the graph may have changed
        // across an --accept-workflow-change resume.
        let expected = ctx
            .ports
            .store
            .review_expected(&review_run_id)
            .await?
            .ok_or_else(|| {
                CoreError::Invariant(format!(
                    "review run {} vanished before resume",
                    review_run_id.as_str()
                ))
            })?;
        let round = ctx
            .ports
            .store
            .review_round(&review_run_id)
            .await?
            .ok_or_else(|| {
                CoreError::Invariant(format!(
                    "review run {} vanished before resume",
                    review_run_id.as_str()
                ))
            })?;
        if collected >= expected {
            Ok(HandlerStep::Done(Outcome::success()))
        } else {
            Ok(HandlerStep::Park(ParkReason::ReviewVerdicts {
                review_run_id,
                expected,
                round,
            }))
        }
    }
}

async fn persist_reviewer_worktrees(
    ctx: &HandlerCtx<'_>,
    review_run_id: &ReviewRunId,
    reviewer_worktrees: &[ReviewerWorktree],
) -> Result<(), CoreError> {
    for reviewer in reviewer_worktrees {
        if let Some(key) = reviewer.release_key.as_deref() {
            ctx.ports
                .store
                .set_review_worktree(review_run_id, &reviewer.agent_id, &reviewer.worktree)
                .await
                .map_err(|err| {
                    CoreError::Store(format!(
                        "persist reviewer worktree {key} before spawn failed: {err}"
                    ))
                })?;
        }
    }
    Ok(())
}

fn review_worktree<'a>(ctx: &'a HandlerCtx<'_>) -> &'a Path {
    ctx.context
        .0
        .get("worktree")
        .and_then(serde_json::Value::as_str)
        .map_or_else(|| Path::new("."), Path::new)
}

fn delphi_round(ctx: &HandlerCtx<'_>) -> u32 {
    if ctx.node_attr("visibility") != Some("delphi") {
        return 1;
    }
    ctx.context
        .get("delphi_next_round")
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|round| *round >= 1)
        .unwrap_or(1)
}

struct ReviewPromptInput<'a, 'ctx> {
    ctx: &'a HandlerCtx<'ctx>,
    review_run_id: &'a ReviewRunId,
    reviewer_id: &'a AgentId,
    allocation: &'a AgentAllocation,
    round: u32,
    context_sha: &'a str,
    stance_query: Option<&'a str>,
    prompt_profile: Option<&'a str>,
    stance_hits: &'a [DrawerHit],
    review_worktree: &'a Path,
}

fn review_prompt(input: &ReviewPromptInput<'_, '_>) -> String {
    let &ReviewPromptInput {
        ctx,
        review_run_id,
        reviewer_id,
        allocation,
        round,
        context_sha,
        stance_query,
        prompt_profile,
        stance_hits,
        review_worktree,
    } = input;
    let mut prompt = base_review_prompt(ctx, round, context_sha);
    append_review_runtime_context(&mut prompt, ctx, review_worktree);
    append_agent_allocation_prompt_context(&mut prompt, allocation);
    append_review_submission_context(&mut prompt, ctx, review_run_id, reviewer_id);
    append_prompt_profile_and_stance(&mut prompt, prompt_profile, stance_query, stance_hits);
    prompt
}

fn queued_review_base_prompt(
    ctx: &HandlerCtx<'_>,
    round: u32,
    context_sha: &str,
    prompt_profile: Option<&str>,
    stance_query: Option<&str>,
    stance_hits: &[DrawerHit],
) -> String {
    let mut prompt = base_review_prompt(ctx, round, context_sha);
    append_prompt_profile_and_stance(&mut prompt, prompt_profile, stance_query, stance_hits);
    prompt
}

fn append_prompt_profile_and_stance(
    prompt: &mut String,
    prompt_profile: Option<&str>,
    stance_query: Option<&str>,
    stance_hits: &[DrawerHit],
) {
    if let Some(profile) = prompt_profile {
        let _ = writeln!(prompt);
        let _ = writeln!(prompt, "prompt_profile: {profile}");
    }
    if let Some(query) = stance_query {
        let _ = writeln!(prompt);
        let _ = writeln!(prompt, "stance_pack_query: {query}");
        let _ = writeln!(prompt, "stance_pack:");
        if stance_hits.is_empty() {
            let _ = writeln!(prompt, "- <empty>");
        } else {
            for hit in stance_hits {
                let _ = writeln!(prompt, "- [{}] {}", hit.drawer_id, hit.body);
            }
        }
    }
}

fn append_review_runtime_context(
    prompt: &mut String,
    ctx: &HandlerCtx<'_>,
    review_worktree: &Path,
) {
    if !prompt.is_empty() && !prompt.ends_with('\n') {
        prompt.push('\n');
    }
    let cwd = std::env::current_dir().map_or_else(
        |_| "<unknown>".to_string(),
        |path| path.display().to_string(),
    );
    let _ = writeln!(prompt, "agentd_daemon_cwd: {cwd}");
    let _ = writeln!(
        prompt,
        "agentd_runtime_path_rule: relative paths in this prompt resolve from agentd_daemon_cwd; review code in review_worktree."
    );
    for key in ["spec_path", "plan_path"] {
        if let Some(value) = ctx.context.get(key).and_then(serde_json::Value::as_str) {
            let _ = writeln!(prompt, "{key}: {value}");
        }
    }
    if let Some(worktree) = ctx
        .context
        .get("worktree")
        .and_then(serde_json::Value::as_str)
    {
        let _ = writeln!(prompt, "implementation_worktree: {worktree}");
    }
    let _ = writeln!(
        prompt,
        "review_worktree: {}",
        review_worktree.to_string_lossy()
    );
    let _ = writeln!(
        prompt,
        "agentd_review_task: review the current worktree against the listed spec and plan, then submit pass|concern|blocker with findings."
    );
}

fn append_review_submission_context(
    prompt: &mut String,
    ctx: &HandlerCtx<'_>,
    review_run_id: &ReviewRunId,
    reviewer_id: &AgentId,
) {
    if !prompt.is_empty() && !prompt.ends_with('\n') {
        prompt.push('\n');
    }
    let _ = writeln!(prompt, "agentd_run_id: {}", ctx.run_id.as_str());
    let _ = writeln!(prompt, "agentd_node_id: {}", ctx.node.id);
    let _ = writeln!(prompt, "agentd_reviewer_id: {}", reviewer_id.as_str());
    let _ = writeln!(prompt, "agentd_review_run_id: {}", review_run_id.as_str());
    let _ = writeln!(
        prompt,
        "agentd_submit_review: use JSON-RPC tools/call name=submit_review arguments={{review_run_id:\"{}\",reviewer_id:\"{}\",verdict:\"pass|concern|blocker\",findings:[]}}",
        review_run_id.as_str(),
        reviewer_id.as_str()
    );
}

fn base_review_prompt(ctx: &HandlerCtx<'_>, round: u32, context_sha: &str) -> String {
    if ctx.node_attr("visibility") != Some("delphi") || round <= 1 {
        return format!("adversarial review (context_sha={context_sha})");
    }
    let previous = ctx
        .context
        .get("delphi_previous_verdicts")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<none>");
    format!(
        "adversarial review (context_sha={context_sha}); Delphi round {round}; previous verdicts: {previous}"
    )
}

fn reviewer_map(
    ctx: &HandlerCtx<'_>,
    attr: &str,
    roles: &[String],
) -> Result<Option<BTreeMap<String, String>>, CoreError> {
    let Some(raw) = ctx.node_attr(attr) else {
        return Ok(None);
    };
    let mut map = BTreeMap::new();
    for entry in raw
        .split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let Some((reviewer, value)) = entry.split_once('=') else {
            return Err(CoreError::Invariant(format!(
                "fan_out node '{}' {attr} entry '{entry}' must use reviewer=value",
                ctx.node.id
            )));
        };
        let reviewer = reviewer.trim();
        let value = value.trim();
        if reviewer.is_empty() || value.is_empty() {
            return Err(CoreError::Invariant(format!(
                "fan_out node '{}' {attr} entry '{entry}' must have non-empty reviewer and value",
                ctx.node.id
            )));
        }
        if map
            .insert(reviewer.to_string(), value.to_string())
            .is_some()
        {
            return Err(CoreError::Invariant(format!(
                "fan_out node '{}' {attr} declares reviewer '{reviewer}' more than once",
                ctx.node.id
            )));
        }
    }

    for role in roles {
        if !map.contains_key(role) {
            return Err(CoreError::Invariant(format!(
                "fan_out node '{}' {attr} missing reviewer '{role}'",
                ctx.node.id
            )));
        }
    }
    for reviewer in map.keys() {
        if !roles.iter().any(|role| role == reviewer) {
            return Err(CoreError::Invariant(format!(
                "fan_out node '{}' {attr} references unknown reviewer '{reviewer}'",
                ctx.node.id
            )));
        }
    }
    Ok(Some(map))
}

fn ensure_distinct_stance_queries(
    ctx: &HandlerCtx<'_>,
    stance_queries: Option<&BTreeMap<String, String>>,
) -> Result<(), CoreError> {
    let Some(stance_queries) = stance_queries else {
        return Ok(());
    };
    let mut seen = BTreeSet::new();
    for (reviewer, query) in stance_queries {
        if !seen.insert(query.as_str()) {
            return Err(CoreError::Invariant(format!(
                "fan_out node '{}' requires distinct stance_queries; query '{query}' is reused by reviewer '{reviewer}'",
                ctx.node.id
            )));
        }
    }
    Ok(())
}

async fn stance_packs(
    ctx: &HandlerCtx<'_>,
    roles: &[String],
    stance_queries: Option<&BTreeMap<String, String>>,
) -> BTreeMap<String, Vec<DrawerHit>> {
    let Some(stance_queries) = stance_queries else {
        return BTreeMap::new();
    };
    let mut packs = BTreeMap::new();
    for role in roles {
        let Some(query) = stance_queries.get(role) else {
            continue;
        };
        let hits = match ctx.ports.mempal.search(query, "project", "").await {
            Ok(hits) => hits,
            Err(err) => {
                tracing::warn!(
                    reviewer = role.as_str(),
                    query = query.as_str(),
                    error = %err,
                    "reviewer stance-pack mempal search failed"
                );
                Vec::new()
            }
        };
        packs.insert(role.clone(), hits);
    }
    packs
}

#[derive(Debug)]
struct ReviewerWorktree {
    role: String,
    agent_id: AgentId,
    allocation: AgentAllocation,
    worktree: PathBuf,
    release_key: Option<String>,
}

async fn reviewer_worktrees(
    ctx: &HandlerCtx<'_>,
    review_run_id: &ReviewRunId,
    roles: &[String],
    round: u32,
    context_sha: &str,
) -> Result<Vec<ReviewerWorktree>, CoreError> {
    let Some(source) = ctx
        .context
        .0
        .get("worktree")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
    else {
        let fallback = review_worktree(ctx).to_path_buf();
        return allocated_worktrees(
            ctx,
            review_run_id,
            roles,
            None,
            &fallback,
            round,
            context_sha,
        )
        .await;
    };
    let Some(allocator) = ctx.worktree_allocator() else {
        return allocated_worktrees(ctx, review_run_id, roles, None, &source, round, context_sha)
            .await;
    };

    allocated_worktrees(
        ctx,
        review_run_id,
        roles,
        Some((allocator, source.clone())),
        &source,
        round,
        context_sha,
    )
    .await
}

async fn allocated_worktrees(
    ctx: &HandlerCtx<'_>,
    review_run_id: &ReviewRunId,
    roles: &[String],
    snapshot: Option<(&dyn crate::ports::WorktreeAllocator, PathBuf)>,
    fallback: &Path,
    round: u32,
    context_sha: &str,
) -> Result<Vec<ReviewerWorktree>, CoreError> {
    let mut out = Vec::with_capacity(roles.len());
    for role in roles {
        let allocation = allocate_reviewer(ctx, review_run_id, role, round, context_sha).await?;
        let agent_id = allocation.agent_id.clone();
        let (worktree, release_key) = if let Some((allocator, source)) = snapshot.as_ref() {
            if allocation.status == AgentAllocationStatus::Queued {
                (source.clone(), None)
            } else {
                let key = reviewer_worktree_key(review_run_id, &agent_id);
                (
                    allocator.allocate_snapshot(&key, source.as_path()).await?,
                    Some(key),
                )
            }
        } else {
            (fallback.to_path_buf(), None)
        };
        out.push(ReviewerWorktree {
            role: role.clone(),
            agent_id,
            allocation,
            worktree,
            release_key,
        });
    }
    Ok(out)
}

async fn allocate_reviewer(
    ctx: &HandlerCtx<'_>,
    review_run_id: &ReviewRunId,
    role: &str,
    round: u32,
    context_sha: &str,
) -> Result<AgentAllocation, CoreError> {
    ctx.ports
        .agent_allocator
        .allocate(AgentAllocationRequest {
            run_id: ctx.run_id.clone(),
            node_id: NodeId::parsed(&ctx.node.id),
            role: role.to_string(),
            capability: ctx.node_attr("capability").map(ToString::to_string),
            task: serde_json::json!({
                "handler": "parallel.fan_out",
                "kind": "workflow_fan_out_reviewer",
                "runId": ctx.run_id.as_str(),
                "nodeId": ctx.node.id,
                "reviewRunId": review_run_id.as_str(),
                "requestedRole": role,
                "round": round,
                "contextSha": context_sha,
            }),
        })
        .await
}

fn stage_review_run_context(
    ctx: &mut HandlerCtx<'_>,
    review_run_id: &ReviewRunId,
    context_sha: &str,
    round: u32,
) {
    ctx.stage(
        "review_run_id",
        serde_json::Value::String(review_run_id.as_str().to_string()),
    );
    ctx.stage(
        "context_sha",
        serde_json::Value::String(context_sha.to_string()),
    );
    ctx.stage(
        "review_round",
        serde_json::Value::Number(serde_json::Number::from(round)),
    );
}

fn stage_queued_fan_out_dispatch(
    ctx: &mut HandlerCtx<'_>,
    review_run_id: &ReviewRunId,
    role: &str,
    round: u32,
    context_sha: &str,
    base_prompt: &str,
    source_worktree: &Path,
) {
    let mut root = ctx
        .staged_updates()
        .get("agentd_queued_workflow_dispatches")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut node_entry = root
        .remove(&ctx.node.id)
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let mut reviewers = node_entry
        .remove("reviewers")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    reviewers.insert(
        role.to_string(),
        serde_json::json!({
            "handler": "parallel.fan_out",
            "reviewRunId": review_run_id.as_str(),
            "requestedRole": role,
            "round": round,
            "contextSha": context_sha,
            "basePrompt": base_prompt,
            "sourceWorktree": source_worktree.to_string_lossy(),
        }),
    );
    node_entry.insert(
        "handler".to_string(),
        serde_json::Value::String("parallel.fan_out".to_string()),
    );
    node_entry.insert(
        "reviewers".to_string(),
        serde_json::Value::Object(reviewers),
    );
    root.insert(ctx.node.id.clone(), serde_json::Value::Object(node_entry));
    ctx.stage(
        "agentd_queued_workflow_dispatches",
        serde_json::Value::Object(root),
    );
}

fn reviewer_worktree_key(review_run_id: &ReviewRunId, reviewer_id: &AgentId) -> String {
    format!("review-{}-{}", review_run_id.as_str(), reviewer_id.as_str())
}

async fn release_reviewer_worktree(
    ctx: &HandlerCtx<'_>,
    review_run_id: &ReviewRunId,
    reviewer_id: &AgentId,
) -> Result<(), CoreError> {
    let Some(allocator) = ctx.worktree_allocator() else {
        return Ok(());
    };
    let Some(path) = ctx
        .ports
        .store
        .take_review_worktree(review_run_id, reviewer_id)
        .await?
    else {
        return Ok(());
    };
    let key = reviewer_worktree_key(review_run_id, reviewer_id);
    if let Err(err) = allocator.release(&key, &path).await {
        tracing::warn!(
            review_run_id = %review_run_id.as_str(),
            reviewer_id = %reviewer_id.as_str(),
            worktree = %path.display(),
            error = %err,
            "reviewer worktree release failed; boot-GC remains the fallback"
        );
    }
    Ok(())
}
