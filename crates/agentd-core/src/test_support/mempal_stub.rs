//! An in-memory [`MempalClient`]. Returns scripted search/fact-check hits and
//! records ingest/kg writes. Touches no on-disk mempal store — purely in memory.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::CoreError;
use crate::ports::{DrawerHit, MempalClient};

/// Scripted, recording mempal client for tests.
#[derive(Debug, Default)]
pub struct MempalStub {
    hits: Mutex<Vec<DrawerHit>>,
    query_hits: Mutex<HashMap<String, Vec<DrawerHit>>>,
    searches: Mutex<Vec<(String, String, String)>>,
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

    /// Script hits returned only for one `search` query.
    pub fn set_query_hits(&self, query: impl Into<String>, hits: Vec<DrawerHit>) {
        self.query_hits
            .lock()
            .expect("query hits lock")
            .insert(query.into(), hits);
    }

    /// Recorded `(query, wing, kind)` search calls.
    #[must_use]
    pub fn searches(&self) -> Vec<(String, String, String)> {
        self.searches.lock().expect("searches lock").clone()
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
        query: &str,
        wing: &str,
        kind: &str,
    ) -> Result<Vec<DrawerHit>, CoreError> {
        self.searches.lock().expect("searches lock").push((
            query.to_string(),
            wing.to_string(),
            kind.to_string(),
        ));
        if let Some(hits) = self
            .query_hits
            .lock()
            .expect("query hits lock")
            .get(query)
            .cloned()
        {
            return Ok(hits);
        }
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
