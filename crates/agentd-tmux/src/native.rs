//! Native PTY process runtime for the agentd worker.
//!
//! This module owns only disposable host resources: the child process, PTY,
//! bounded output ring, and provider-native reference. Durable runtime session
//! and attempt state belongs to `agentd-store` and is updated by the caller.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
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
        let mut reader = pair
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
        let reader_shared = Arc::clone(&shared);
        let capacity = config.output_capacity.max(1);
        thread::Builder::new()
            .name("agentd-native-pty-reader".into())
            .spawn(move || {
                let mut buf = [0_u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(read) => {
                            if let Ok(mut state) = reader_shared.state.lock() {
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
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| NativeRuntimeError::Pty(e.to_string()))?;

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
                let mut state = match wait_shared.state.lock() {
                    Ok(state) => state,
                    Err(_) => return,
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
            .map(|state| state.status)
            .unwrap_or(NativeProcessStatus::Gone)
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
