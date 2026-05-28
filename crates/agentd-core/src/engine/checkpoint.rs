//! Durable per-run checkpoint + resume sha policy (design §3.3).
//! See `specs/core/p4-checkpoint-and-resume.spec.md`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::CoreError;
use crate::types::{NodeId, RunContext, RunId};

/// A snapshot of run state, written after every node so a crash can resume.
///
/// `retry_counts` is a `BTreeMap` (not `HashMap`) so the serialized key order is
/// deterministic. `context_snapshot` reflects everything a handler staged before
/// it returned (including before a Park), so nothing in-flight is lost.
// No `Eq`: context_snapshot (RunContext) contains serde_json::Value (f64).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub run_id: RunId,
    pub current_node: NodeId,
    pub completed_nodes: Vec<NodeId>,
    pub retry_counts: BTreeMap<NodeId, u32>,
    pub context_snapshot: RunContext,
    pub workflow_sha: String,
}

impl Checkpoint {
    /// Write the checkpoint atomically: serialize to `<path>.tmp`, then rename
    /// over `path` (POSIX rename is atomic on the same filesystem).
    ///
    /// # Errors
    /// Returns [`CoreError::Io`] / [`CoreError::Serde`] on write or encode failure.
    pub fn write_atomic(&self, path: &Path) -> Result<(), CoreError> {
        let json = serde_json::to_vec_pretty(self)?;
        let tmp = tmp_path(path);
        std::fs::write(&tmp, &json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Load a checkpoint from disk.
    ///
    /// # Errors
    /// Returns [`CoreError::Io`] / [`CoreError::Serde`] on read or decode failure.
    pub fn load(path: &Path) -> Result<Self, CoreError> {
        let bytes = std::fs::read(path)?;
        let cp = serde_json::from_slice(&bytes)?;
        Ok(cp)
    }

    /// Enforce the resume sha policy (design §3.3): a matching sha resumes; a
    /// changed sha requires `accept_change=true` (and logs a warning), else errors.
    ///
    /// # Errors
    /// Returns [`CoreError::WorkflowShaChanged`] when the sha differs and
    /// `accept_change` is false.
    pub fn resume_guard(&self, current_sha: &str, accept_change: bool) -> Result<(), CoreError> {
        if self.workflow_sha == current_sha {
            return Ok(());
        }
        if accept_change {
            tracing::warn!(
                stored = %self.workflow_sha,
                current = %current_sha,
                "resuming across a changed workflow_sha (--accept-workflow-change)"
            );
            return Ok(());
        }
        Err(CoreError::WorkflowShaChanged)
    }
}

fn tmp_path(path: &Path) -> std::path::PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".tmp");
    std::path::PathBuf::from(s)
}
