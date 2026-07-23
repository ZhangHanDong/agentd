//! Native PTY process runtime for the agentd worker.
//!
//! This module owns only disposable host resources: the child process, PTY,
//! bounded output ring, and provider-native reference. Durable runtime session
//! and attempt state belongs to `agentd-store` and is updated by the caller.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct NativeProcessConfig {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Vec<(String, String)>,
    pub output_capacity: usize,
    pub native_session_ref: Option<String>,
}

impl Default for NativeProcessConfig {
    fn default() -> Self {
        Self {
            program: String::new(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            output_capacity: 64 * 1024,
            native_session_ref: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeProcessStatus {
    Running,
    Exited { code: Option<i32> },
    Gone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeProcessEvent {
    Exited { code: Option<i32>, output: Vec<u8> },
    Gone { reason: String, output: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NativeSpoolRecord {
    pub storage_ref: String,
    pub content_sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Error)]
pub enum NativeRuntimeError {
    #[error("native runtime program is empty")]
    EmptyProgram,
    #[error("native PTY setup failed: {0}")]
    Pty(String),
    #[error("native PTY child spawn failed: {0}")]
    Spawn(String),
    #[error("native runtime wait timed out")]
    Timeout,
    #[error("native runtime lock poisoned")]
    Poisoned,
    #[error("native runtime is already terminal")]
    AlreadyTerminal,
}

#[derive(Debug)]
struct RuntimeState {
    status: NativeProcessStatus,
    output: VecDeque<u8>,
    event: Option<NativeProcessEvent>,
    native_session_ref: Option<String>,
}

#[derive(Debug)]
struct SharedState {
    state: Mutex<RuntimeState>,
    changed: Condvar,
}

pub struct NativeRuntime {
    shared: Arc<SharedState>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Arc<Mutex<Option<Box<dyn Child + Send + Sync>>>>,
}

impl std::fmt::Debug for NativeRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeRuntime")
            .field("shared", &self.shared)
            .finish_non_exhaustive()
    }
}

impl NativeRuntime {
    pub fn spawn(config: NativeProcessConfig) -> Result<Self, NativeRuntimeError> {
        if config.program.trim().is_empty() {
            return Err(NativeRuntimeError::EmptyProgram);
        }
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| NativeRuntimeError::Pty(e.to_string()))?;

        let mut command = CommandBuilder::new(&config.program);
        command.args(&config.args);
        if let Some(cwd) = &config.cwd {
            command.cwd(cwd);
        }
        for (key, value) in &config.env {
            command.env(key, value);
        }
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| NativeRuntimeError::Spawn(e.to_string()))?;
        let child = Arc::new(Mutex::new(Some(child)));
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| NativeRuntimeError::Pty(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| NativeRuntimeError::Pty(e.to_string()))?;
        let shared = Arc::new(SharedState {
            state: Mutex::new(RuntimeState {
                status: NativeProcessStatus::Running,
                output: VecDeque::with_capacity(config.output_capacity),
                event: None,
                native_session_ref: config.native_session_ref,
            }),
            changed: Condvar::new(),
        });
        Self::spawn_output_reader(reader, Arc::clone(&shared), config.output_capacity.max(1))?;

        let wait_shared = Arc::clone(&shared);
        let wait_child = Arc::clone(&child);
        thread::Builder::new()
            .name("agentd-native-process-waiter".into())
            .spawn(move || {
                let result = wait_child
                    .lock()
                    .ok()
                    .and_then(|mut child| child.take())
                    .map_or_else(
                        || Err(std::io::Error::other("child handle unavailable")),
                        |mut child| child.wait(),
                    );
                let Ok(mut state) = wait_shared.state.lock() else {
                    return;
                };
                let output = state.output.iter().copied().collect::<Vec<_>>();
                match result {
                    Ok(status) => {
                        let code = status.exit_code().try_into().ok();
                        state.status = NativeProcessStatus::Exited { code };
                        state.event = Some(NativeProcessEvent::Exited { code, output });
                    }
                    Err(error) => {
                        state.status = NativeProcessStatus::Gone;
                        state.event = Some(NativeProcessEvent::Gone {
                            reason: error.to_string(),
                            output,
                        });
                    }
                }
                wait_shared.changed.notify_all();
            })
            .map_err(|e| NativeRuntimeError::Pty(e.to_string()))?;

        Ok(Self {
            shared,
            writer: Arc::new(Mutex::new(writer)),
            child,
        })
    }

    #[must_use]
    pub fn status(&self) -> NativeProcessStatus {
        self.shared
            .state
            .lock()
            .map_or(NativeProcessStatus::Gone, |state| state.status)
    }

    #[must_use]
    pub fn is_terminal(&self) -> bool {
        !matches!(self.status(), NativeProcessStatus::Running)
    }

    #[must_use]
    pub fn output(&self) -> Vec<u8> {
        self.shared
            .state
            .lock()
            .map(|state| state.output.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Atomically capture the bounded runtime state for observers.
    #[must_use]
    pub fn snapshot(&self) -> (NativeProcessStatus, Option<String>, Vec<u8>) {
        self.shared
            .state
            .lock()
            .map_or((NativeProcessStatus::Gone, None, Vec::new()), |state| {
                (
                    state.status,
                    state.native_session_ref.clone(),
                    state.output.iter().copied().collect(),
                )
            })
    }

    /// Spool the bounded output snapshot atomically for a later artifact upload.
    pub fn spool_output(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<NativeSpoolRecord, NativeRuntimeError> {
        Self::spool_bytes(path, &self.output())
    }

    /// Spool output after replacing checked-out secret material before it is
    /// persisted or hashed into an evidence envelope.
    pub fn spool_output_redacted(
        &self,
        path: impl AsRef<Path>,
        secrets: &[agentd_core::ports::SecretMaterial],
    ) -> Result<NativeSpoolRecord, NativeRuntimeError> {
        let mut output = self.output();
        for secret in secrets {
            if !secret.as_bytes().is_empty() {
                replace_all(&mut output, secret.as_bytes(), b"[REDACTED]");
            }
        }
        Self::spool_bytes(path, &output)
    }

    /// Pump PTY output into the bounded ring buffer on a dedicated thread.
    fn spawn_output_reader(
        mut reader: Box<dyn Read + Send>,
        shared: Arc<SharedState>,
        capacity: usize,
    ) -> Result<(), NativeRuntimeError> {
        thread::Builder::new()
            .name("agentd-native-pty-reader".into())
            .spawn(move || {
                let mut buf = [0_u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if let Ok(mut state) = shared.state.lock() {
                                for byte in &buf[..read] {
                                    if state.output.len() == capacity {
                                        state.output.pop_front();
                                    }
                                    state.output.push_back(*byte);
                                }
                            } else {
                                break;
                            }
                        }
                    }
                }
            })
            .map_err(|e| NativeRuntimeError::Pty(e.to_string()))?;
        Ok(())
    }

    fn spool_bytes(
        path: impl AsRef<Path>,
        output: &[u8],
    ) -> Result<NativeSpoolRecord, NativeRuntimeError> {
        let path = path.as_ref();
        let parent = path
            .parent()
            .ok_or_else(|| NativeRuntimeError::Pty("spool path has no parent".to_string()))?;
        std::fs::create_dir_all(parent)
            .map_err(|error| NativeRuntimeError::Pty(error.to_string()))?;
        let temp = path.with_extension("part");
        let content_sha256 = format!("{:x}", Sha256::digest(output));
        std::fs::write(&temp, output)
            .map_err(|error| NativeRuntimeError::Pty(error.to_string()))?;
        std::fs::rename(&temp, path).map_err(|error| NativeRuntimeError::Pty(error.to_string()))?;
        Ok(NativeSpoolRecord {
            storage_ref: path.to_string_lossy().into_owned(),
            content_sha256,
            size_bytes: output.len() as u64,
        })
    }

    #[must_use]
    pub fn native_session_ref(&self) -> Option<String> {
        self.shared
            .state
            .lock()
            .ok()
            .and_then(|state| state.native_session_ref.clone())
    }

    pub fn write(&self, input: &[u8]) -> Result<(), NativeRuntimeError> {
        self.writer
            .lock()
            .map_err(|_| NativeRuntimeError::Poisoned)?
            .write_all(input)
            .map_err(|e| NativeRuntimeError::Pty(e.to_string()))
    }

    /// Terminate the child process and let the waiter reconcile its exit event.
    pub fn terminate(&self) -> Result<(), NativeRuntimeError> {
        let mut child = self
            .child
            .lock()
            .map_err(|_| NativeRuntimeError::Poisoned)?;
        let Some(child) = child.as_mut() else {
            return Err(NativeRuntimeError::AlreadyTerminal);
        };
        child
            .kill()
            .map_err(|error| NativeRuntimeError::Pty(error.to_string()))
    }

    pub fn wait(&self, timeout: Duration) -> Result<NativeProcessEvent, NativeRuntimeError> {
        let deadline = Instant::now() + timeout;
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| NativeRuntimeError::Poisoned)?;
        loop {
            if let Some(event) = &state.event {
                return Ok(event.clone());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(NativeRuntimeError::Timeout);
            }
            let (next, result) = self
                .shared
                .changed
                .wait_timeout(state, remaining)
                .map_err(|_| NativeRuntimeError::Poisoned)?;
            state = next;
            if result.timed_out() {
                return Err(NativeRuntimeError::Timeout);
            }
        }
    }
}

fn replace_all(output: &mut Vec<u8>, needle: &[u8], replacement: &[u8]) {
    if needle.is_empty() {
        return;
    }
    let mut cursor = 0;
    while let Some(relative) = output[cursor..]
        .windows(needle.len())
        .position(|window| window == needle)
    {
        let start = cursor + relative;
        output.splice(start..start + needle.len(), replacement.iter().copied());
        cursor = start + replacement.len();
    }
}

#[cfg(test)]
mod tests {
    use super::replace_all;

    #[test]
    fn redaction_replaces_all_occurrences() {
        let mut output = b"before-secret-after-secret".to_vec();
        replace_all(&mut output, b"secret", b"[REDACTED]");
        assert_eq!(output, b"before-[REDACTED]-after-[REDACTED]");
    }

    #[test]
    fn redaction_ignores_empty_secret() {
        let mut output = b"unchanged".to_vec();
        replace_all(&mut output, b"", b"[REDACTED]");
        assert_eq!(output, b"unchanged");
    }
}
