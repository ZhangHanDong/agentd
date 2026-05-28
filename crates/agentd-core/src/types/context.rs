//! Shared workflow context. Persisted as JSON; merged by `context_updates` after each node.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunContext(pub serde_json::Map<String, serde_json::Value>);

impl RunContext {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a JSON merge patch (shallow).
    pub fn merge(&mut self, patch: &serde_json::Map<String, serde_json::Value>) {
        for (k, v) in patch {
            self.0.insert(k.clone(), v.clone());
        }
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.0.get(key)
    }

    pub fn set(&mut self, key: impl Into<String>, value: serde_json::Value) {
        self.0.insert(key.into(), value);
    }
}
