//! Immutable content-addressed storage for redacted runtime transcripts.

use std::path::{Path, PathBuf};

use agentd_core::ports::{NativeRuntimeError, RuntimeArchivePort, RuntimeTranscriptRef};
use agentd_core::types::{RuntimeAttemptId, RuntimeSessionId, RuntimeTranscriptId};
use sha2::{Digest, Sha256};

/// Filesystem transcript object store rooted at an operator-controlled path.
#[derive(Clone)]
pub struct ContentAddressedTranscriptStore {
    root: PathBuf,
    max_object_bytes: u64,
}

impl std::fmt::Debug for ContentAddressedTranscriptStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ContentAddressedTranscriptStore")
            .field("root", &self.root)
            .field("max_object_bytes", &self.max_object_bytes)
            .finish()
    }
}

impl ContentAddressedTranscriptStore {
    pub fn new(root: impl AsRef<Path>, max_object_bytes: u64) -> Result<Self, NativeRuntimeError> {
        if max_object_bytes == 0 {
            return Err(NativeRuntimeError::Invalid(
                "transcript archive bound must be positive".to_string(),
            ));
        }
        std::fs::create_dir_all(root.as_ref()).map_err(archive_unavailable)?;
        let root = root.as_ref().canonicalize().map_err(archive_unavailable)?;
        std::fs::create_dir_all(root.join("sha256")).map_err(archive_unavailable)?;
        std::fs::create_dir_all(root.join("tmp")).map_err(archive_unavailable)?;
        Ok(Self {
            root,
            max_object_bytes,
        })
    }

    #[must_use]
    pub fn object_path(&self, content_sha256: &str) -> PathBuf {
        let prefix = content_sha256.get(..2).unwrap_or("invalid");
        self.root.join("sha256").join(prefix).join(content_sha256)
    }
}

#[async_trait::async_trait]
impl RuntimeArchivePort for ContentAddressedTranscriptStore {
    async fn archive_runtime_transcript(
        &self,
        _session_id: &RuntimeSessionId,
        _attempt_id: &RuntimeAttemptId,
        redacted_transcript: &[u8],
        truncated: bool,
        observed_at: i64,
    ) -> Result<RuntimeTranscriptRef, NativeRuntimeError> {
        if observed_at < 0 || redacted_transcript.len() as u64 > self.max_object_bytes {
            return Err(NativeRuntimeError::Invalid(
                "transcript archive request exceeds bounds".to_string(),
            ));
        }
        let content_sha256 = hex::encode(Sha256::digest(redacted_transcript));
        let destination = self.object_path(&content_sha256);
        let parent = destination.parent().ok_or_else(|| {
            NativeRuntimeError::Unavailable("invalid transcript object path".to_string())
        })?;
        std::fs::create_dir_all(parent).map_err(archive_unavailable)?;
        if destination.exists() {
            let existing = std::fs::read(&destination).map_err(archive_unavailable)?;
            if existing != redacted_transcript {
                return Err(NativeRuntimeError::Conflict(
                    "content-addressed transcript collision".to_string(),
                ));
            }
        } else {
            let temporary = self
                .root
                .join("tmp")
                .join(format!("{}.part", RuntimeTranscriptId::new()));
            std::fs::write(&temporary, redacted_transcript).map_err(archive_unavailable)?;
            match std::fs::rename(&temporary, &destination) {
                Ok(()) => {}
                Err(_error) if destination.exists() => {
                    let _ = std::fs::remove_file(&temporary);
                    let existing = std::fs::read(&destination).map_err(archive_unavailable)?;
                    if existing != redacted_transcript {
                        return Err(NativeRuntimeError::Conflict(
                            "content-addressed transcript collision".to_string(),
                        ));
                    }
                }
                Err(error) => {
                    let _ = std::fs::remove_file(&temporary);
                    return Err(archive_unavailable(error));
                }
            }
        }
        Ok(RuntimeTranscriptRef {
            id: RuntimeTranscriptId::new(),
            content_sha256: content_sha256.clone(),
            storage_ref: format!("sha256:{content_sha256}"),
            size_bytes: redacted_transcript.len() as u64,
            truncated,
            archived_at: observed_at,
        })
    }
}

fn archive_unavailable(error: impl std::fmt::Display) -> NativeRuntimeError {
    NativeRuntimeError::Unavailable(format!("transcript archive unavailable: {error}"))
}
