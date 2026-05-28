//! An in-memory [`MempalClient`]. Returns scripted search/fact-check hits and
//! records ingest/kg writes. Touches no on-disk mempal store — purely in memory.

use std::sync::Mutex;

use crate::CoreError;
use crate::ports::{DrawerHit, MempalClient};

/// Scripted, recording mempal client for tests.
#[derive(Debug, Default)]
pub struct MempalStub {
    hits: Mutex<Vec<DrawerHit>>,
    ingested: Mutex<Vec<(String, String, String)>>,
    kg_triples: Mutex<Vec<(String, String, String)>>,
}

impl MempalStub {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Script the hits returned by `search`/`fact_check`.
    pub fn set_hits(&self, hits: Vec<DrawerHit>) {
        *self.hits.lock().expect("hits lock") = hits;
    }

    /// Recorded `(wing, kind, body)` ingest calls.
    #[must_use]
    pub fn ingested(&self) -> Vec<(String, String, String)> {
        self.ingested.lock().expect("ingested lock").clone()
    }

    /// Recorded `(subject, predicate, object)` kg triples.
    #[must_use]
    pub fn kg_triples(&self) -> Vec<(String, String, String)> {
        self.kg_triples.lock().expect("kg lock").clone()
    }
}

#[async_trait::async_trait]
impl MempalClient for MempalStub {
    async fn search(
        &self,
        _query: &str,
        _wing: &str,
        _kind: &str,
    ) -> Result<Vec<DrawerHit>, CoreError> {
        Ok(self.hits.lock().expect("hits lock").clone())
    }

    async fn ingest(&self, wing: &str, kind: &str, body: &str) -> Result<(), CoreError> {
        self.ingested.lock().expect("ingested lock").push((
            wing.to_string(),
            kind.to_string(),
            body.to_string(),
        ));
        Ok(())
    }

    async fn kg_add(&self, subject: &str, predicate: &str, object: &str) -> Result<(), CoreError> {
        self.kg_triples.lock().expect("kg lock").push((
            subject.to_string(),
            predicate.to_string(),
            object.to_string(),
        ));
        Ok(())
    }

    async fn fact_check(&self, _claim: &str) -> Result<Vec<DrawerHit>, CoreError> {
        Ok(self.hits.lock().expect("hits lock").clone())
    }
}
