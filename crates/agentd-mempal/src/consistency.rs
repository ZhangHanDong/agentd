//! Read-only consistency check (design §3.5): report drawers agentd believes it
//! ingested that mempal does not surface. It reports drift and never re-ingests
//! — on a git↔mempal conflict, git wins and re-ingest is a separate action.

use agentd_core::ports::MempalClient;

/// A drawer agentd expects mempal to surface (e.g. a drained Ingest write).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedDrawer {
    pub wing: String,
    pub kind: String,
    pub query: String,
}

/// Search mempal for each `expected` drawer and return the ones it does not
/// surface. A search failure conservatively counts the drawer as missing (and
/// logs). This makes no writes and never errors — it is a best-effort report.
pub async fn check_against(
    expected: &[ExpectedDrawer],
    client: &dyn MempalClient,
) -> Vec<ExpectedDrawer> {
    let mut missing = Vec::new();
    for drawer in expected {
        let present = match client
            .search(&drawer.query, &drawer.wing, &drawer.kind)
            .await
        {
            Ok(hits) => !hits.is_empty(),
            Err(e) => {
                tracing::warn!(
                    query = %drawer.query,
                    error = %e,
                    "consistency search failed; conservatively treating the drawer as missing"
                );
                false
            }
        };
        if !present {
            missing.push(drawer.clone());
        }
    }
    missing
}
