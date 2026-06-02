//! `review_runs` + `review_verdicts` operations. Verdict insert is idempotent
//! per reviewer (`ON CONFLICT(review_run_id, reviewer_id) DO NOTHING`), and the
//! park-open invariant is `count(verdicts) < expected` (parity with the fake).

use agentd_core::types::{AgentId, NodeId, ReviewRunId, ReviewVerdict, RunId, VerdictValue};
use sqlx::{Row, SqlitePool};

use crate::error::StoreError;
use crate::util::now_unix;

fn verdict_str(v: VerdictValue) -> &'static str {
    match v {
        VerdictValue::Pass => "pass",
        VerdictValue::Fail => "fail",
        VerdictValue::Block => "block",
    }
}

fn parse_verdict(s: &str) -> Result<VerdictValue, StoreError> {
    Ok(match s {
        "pass" => VerdictValue::Pass,
        "fail" => VerdictValue::Fail,
        "block" => VerdictValue::Block,
        other => return Err(StoreError::Invariant(format!("unknown verdict '{other}'"))),
    })
}

/// Insert a review run, returning its generated id.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure (e.g. an unknown run FK).
pub async fn insert_review_run(
    pool: &SqlitePool,
    run_id: &RunId,
    node_id: &NodeId,
    expected: usize,
    context_sha: &str,
) -> Result<ReviewRunId, StoreError> {
    let id = format!("rr_{}", ulid::Ulid::new());
    sqlx::query(
        "INSERT INTO review_runs (id, run_id, node_id, expected, context_sha, started_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(run_id.as_str())
    .bind(node_id.as_str())
    .bind(i64::try_from(expected).unwrap_or(i64::MAX))
    .bind(context_sha)
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(ReviewRunId::from_string(id))
}

/// Resolve the parked `(run_id, node_id)` — only while fewer than `expected`
/// verdicts have arrived.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
/// Forward read: the OPEN review run for `run_id` — the one still collecting
/// verdicts (`count(verdicts) < expected`) — as its id, or `None`. The symmetric
/// counterpart of `lookup_park_by_review_run`; the production `RunHost` (and the
/// scriptable agent) resolve which review a verdict belongs to through it. If
/// more than one is open, the most recently started is returned. No migration.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn find_open_review_run(
    pool: &SqlitePool,
    run_id: &RunId,
) -> Result<Option<ReviewRunId>, StoreError> {
    let row = sqlx::query(
        "SELECT id FROM review_runs \
         WHERE run_id = ? \
           AND (SELECT COUNT(*) FROM review_verdicts WHERE review_run_id = review_runs.id) < expected \
         ORDER BY started_at DESC LIMIT 1",
    )
    .bind(run_id.as_str())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| ReviewRunId::from_string(r.get::<String, _>("id"))))
}

pub async fn lookup_park_by_review_run(
    pool: &SqlitePool,
    review_run_id: &ReviewRunId,
) -> Result<Option<(RunId, NodeId)>, StoreError> {
    let row = sqlx::query(
        "SELECT run_id, node_id, expected, \
            (SELECT COUNT(*) FROM review_verdicts WHERE review_run_id = review_runs.id) AS collected \
         FROM review_runs WHERE id = ?",
    )
    .bind(review_run_id.as_str())
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let expected: i64 = row.get("expected");
    let collected: i64 = row.get("collected");
    if collected >= expected {
        return Ok(None); // no longer open — all verdicts in
    }
    Ok(Some((
        RunId::from_string(row.get::<String, _>("run_id")),
        NodeId::from_string(row.get::<String, _>("node_id")),
    )))
}

/// Record a verdict; idempotent per reviewer (a duplicate reviewer is a no-op).
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure (e.g. an unknown review-run FK).
pub async fn insert_review_verdict(
    pool: &SqlitePool,
    review_run_id: &ReviewRunId,
    verdict: ReviewVerdict,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO review_verdicts (review_run_id, reviewer_id, verdict, findings, submitted_at) \
         VALUES (?, ?, ?, '', ?) ON CONFLICT(review_run_id, reviewer_id) DO NOTHING",
    )
    .bind(review_run_id.as_str())
    .bind(verdict.reviewer_id.as_str())
    .bind(verdict_str(verdict.value))
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

/// Count distinct reviewers who have submitted on a review run.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn count_verdicts(
    pool: &SqlitePool,
    review_run_id: &ReviewRunId,
) -> Result<usize, StoreError> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM review_verdicts WHERE review_run_id = ?")
        .bind(review_run_id.as_str())
        .fetch_one(pool)
        .await?;
    Ok(usize::try_from(n).unwrap_or(0))
}

/// The reviewer count a review run waits for, or `None` if unknown.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn review_expected(
    pool: &SqlitePool,
    review_run_id: &ReviewRunId,
) -> Result<Option<usize>, StoreError> {
    let row: Option<i64> = sqlx::query_scalar("SELECT expected FROM review_runs WHERE id = ?")
        .bind(review_run_id.as_str())
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|n| usize::try_from(n).unwrap_or(0)))
}

/// List a review run's verdicts.
///
/// # Errors
/// Returns [`StoreError`] on a decode or database failure.
pub async fn list_verdicts(
    pool: &SqlitePool,
    review_run_id: &ReviewRunId,
) -> Result<Vec<ReviewVerdict>, StoreError> {
    let rows = sqlx::query(
        "SELECT reviewer_id, verdict FROM review_verdicts WHERE review_run_id = ? \
         ORDER BY submitted_at, reviewer_id",
    )
    .bind(review_run_id.as_str())
    .fetch_all(pool)
    .await?;
    let mut verdicts = Vec::with_capacity(rows.len());
    for row in rows {
        verdicts.push(ReviewVerdict {
            reviewer_id: AgentId::parsed(row.get::<String, _>("reviewer_id")),
            value: parse_verdict(&row.get::<String, _>("verdict"))?,
        });
    }
    Ok(verdicts)
}
