//! `RecordingToolCaller` — records tool-call argv and replays scripted JSON
//! results FIFO; a `hang` mode never resolves (to drive the read timeout, §3.4).
//! Compiled only under `test-support`/`cfg(test)`, never in a release binary.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

use crate::error::MempalError;
use crate::transport::McpToolCaller;

/// A recording, scripted [`McpToolCaller`] for tests.
#[derive(Debug, Default)]
pub struct RecordingToolCaller {
    scripted: Mutex<VecDeque<Result<Value, MempalError>>>,
    calls: Mutex<Vec<(String, Value)>>,
    hang: AtomicBool,
}

impl RecordingToolCaller {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue one scripted result (returned by a future `call_tool`, FIFO).
    pub fn push_result(&self, result: Result<Value, MempalError>) {
        self.scripted
            .lock()
            .expect("scripted lock")
            .push_back(result);
    }

    /// When `true`, the next `call_tool` never resolves (drives the timeout).
    pub fn set_hang(&self, hang: bool) {
        self.hang.store(hang, Ordering::SeqCst);
    }

    /// The `(tool, args)` of every recorded call, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().expect("calls lock").clone()
    }
}

#[async_trait::async_trait]
impl McpToolCaller for RecordingToolCaller {
    async fn call_tool(&self, tool: &str, args: Value) -> Result<Value, MempalError> {
        self.calls
            .lock()
            .expect("calls lock")
            .push((tool.to_string(), args));
        if self.hang.load(Ordering::SeqCst) {
            // A genuinely never-resolving future so a zero timeout actually trips.
            std::future::pending::<()>().await;
        }
        self.scripted
            .lock()
            .expect("scripted lock")
            .pop_front()
            .unwrap_or_else(|| Ok(Value::Null))
    }
}
