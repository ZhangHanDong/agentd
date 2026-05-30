//! The background outbox drainer (design §3.4). `drain_once` pulls pending rows
//! FIFO and delivers each to mempal via the `MempalClient`, marking drained on
//! success and retrying (with an attempt bound + operator alert) on failure.
//! Because enqueue already happened in the outcome transaction, a slow or down
//! mempal never stalls workflow execution.

use std::sync::Arc;
use std::time::Duration;

use agentd_core::CoreError;
use agentd_core::ports::MempalClient;
use agentd_core::types::MempalWrite;
use agentd_store::{SqliteStore, outbox_repo};

/// Drainer tuning.
#[derive(Debug, Clone)]
pub struct DrainerConfig {
    /// Rows claimed per pass.
    pub batch_limit: i64,
    /// A row whose `attempts` exceed this is given up on and alerted (§3.4).
    pub max_attempts: i64,
    /// Sleep after a productive pass (the `spawn` loop's base interval).
    pub idle_interval: Duration,
    /// Backoff ceiling when passes are idle or erroring.
    pub max_interval: Duration,
}

impl Default for DrainerConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            max_attempts: 5,
            idle_interval: Duration::from_secs(1),
            max_interval: Duration::from_secs(30),
        }
    }
}

/// What one [`drain_once`] pass did.
#[derive(Debug, Default, Clone)]
pub struct DrainReport {
    /// Rows delivered and marked drained.
    pub drained: usize,
    /// Rows whose delivery failed and were left to retry.
    pub retried: usize,
    /// Outbox ids past the attempt bound (operator action needed).
    pub alerts: Vec<i64>,
}

/// Deliver one write to mempal through the port.
async fn dispatch(client: &dyn MempalClient, write: &MempalWrite) -> Result<(), CoreError> {
    match write {
        MempalWrite::Ingest {
            wing, kind, body, ..
        } => client.ingest(wing, kind, body).await,
        MempalWrite::KgAdd {
            subject,
            predicate,
            object,
        } => client.kg_add(subject, predicate, object).await,
        MempalWrite::FactCheck { text } => client.fact_check(text).await.map(|_| ()),
    }
}

/// Run one drain pass: claim pending rows FIFO, deliver each, mark drained on
/// success, mark failed (and maybe alert) otherwise.
///
/// # Errors
/// [`CoreError::Store`] when the outbox cannot be read or updated. A mempal
/// delivery failure is NOT a `drain_once` error — it is recorded on the row for
/// the next pass to retry.
pub async fn drain_once(
    store: &SqliteStore,
    client: &dyn MempalClient,
    cfg: &DrainerConfig,
) -> Result<DrainReport, CoreError> {
    let pool = store.pool();
    let pending = outbox_repo::claim_pending(pool, cfg.batch_limit).await?;
    let mut report = DrainReport::default();

    for row in pending {
        // Past the bound: stop retrying, keep the row, alert the operator (§3.4).
        if row.attempts > cfg.max_attempts {
            report.alerts.push(row.id);
            continue;
        }
        let write: MempalWrite = match serde_json::from_str(&row.payload) {
            Ok(write) => write,
            Err(e) => {
                outbox_repo::mark_failed(pool, row.id, &format!("decode: {e}")).await?;
                report.retried += 1;
                if row.attempts + 1 > cfg.max_attempts {
                    report.alerts.push(row.id);
                }
                continue;
            }
        };
        match dispatch(client, &write).await {
            Ok(()) => {
                outbox_repo::mark_drained(pool, row.id).await?;
                report.drained += 1;
            }
            Err(e) => {
                outbox_repo::mark_failed(pool, row.id, &e.to_string()).await?;
                report.retried += 1;
                if row.attempts + 1 > cfg.max_attempts {
                    tracing::error!(
                        outbox_id = row.id,
                        attempts = row.attempts + 1,
                        "mempal outbox row exceeded the attempt bound; operator action needed"
                    );
                    report.alerts.push(row.id);
                }
            }
        }
    }
    Ok(report)
}

/// Spawn the background drainer loop (design §3.4): run [`drain_once`] forever,
/// backing off exponentially when a pass is idle or errors.
#[must_use]
pub fn spawn(
    store: Arc<SqliteStore>,
    client: Arc<dyn MempalClient>,
    cfg: DrainerConfig,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = cfg.idle_interval;
        loop {
            match drain_once(&store, client.as_ref(), &cfg).await {
                Ok(report) if report.drained > 0 || report.retried > 0 => {
                    interval = cfg.idle_interval;
                }
                Ok(_) => interval = (interval * 2).min(cfg.max_interval),
                Err(e) => {
                    tracing::error!(error = %e, "outbox drain pass failed");
                    interval = (interval * 2).min(cfg.max_interval);
                }
            }
            tokio::time::sleep(interval).await;
        }
    })
}
