//! `OfflineMempal` — the standalone-mode [`MempalClient`] (no mempal server).
//! The shipped Path-B workflows (draft.dot / execute.dot) do not call mempal,
//! and `codergen`'s memory lookup is best-effort, so an empty/no-op client is
//! the correct standalone default. A real mempal MCP client is a deployment
//! concern (P0.9 checklist). This is production (NOT test-support) so the daemon
//! binary can use it.

use agentd_core::CoreError;
use agentd_core::ports::MempalClient;
use agentd_core::ports::mempal::DrawerHit;

/// A no-op `MempalClient`: searches return nothing, writes succeed silently.
#[derive(Debug, Default, Clone, Copy)]
pub struct OfflineMempal;

#[async_trait::async_trait]
impl MempalClient for OfflineMempal {
    async fn search(
        &self,
        _q: &str,
        _wing: &str,
        _kind: &str,
    ) -> Result<Vec<DrawerHit>, CoreError> {
        Ok(Vec::new())
    }

    async fn ingest(&self, _wing: &str, _kind: &str, _body: &str) -> Result<(), CoreError> {
        Ok(())
    }

    async fn kg_add(&self, _s: &str, _p: &str, _o: &str) -> Result<(), CoreError> {
        Ok(())
    }

    async fn fact_check(&self, _claim: &str) -> Result<Vec<DrawerHit>, CoreError> {
        Ok(Vec::new())
    }
}
