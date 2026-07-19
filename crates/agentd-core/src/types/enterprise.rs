//! Enterprise execution lifecycle states shared by storage and future APIs.

use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use super::{LeaseId, RuntimeSessionId, TaskRunId, WorkerIncarnationId};
use crate::ports::ExecutionSecurityScope;

/// Typed, versioned provider launch input carried with a durable task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeExecutionSpec {
    pub version: u32,
    pub provider: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

impl NativeExecutionSpec {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != 1
            || self.provider.trim().is_empty()
            || self.program.trim().is_empty()
            || self.program.contains('\0')
            || self.args.iter().any(|arg| arg.contains('\0'))
            || self
                .cwd
                .as_deref()
                .is_some_and(|cwd| cwd.is_empty() || cwd.contains('\0'))
            || self
                .env
                .iter()
                .any(|(key, value)| !valid_env_key(key) || value.contains('\0'))
        {
            return Err("invalid native execution spec");
        }
        if !self.provider_matches_program() {
            return Err("execution provider does not match program");
        }
        Ok(())
    }

    #[must_use]
    pub fn provider_matches_program(&self) -> bool {
        let executable = self.program.rsplit('/').next().unwrap_or_default();
        match self.provider.as_str() {
            "codex" => executable == "codex",
            "claude" => matches!(executable, "claude" | "claude-code"),
            _ => false,
        }
    }
}

fn valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(first) if first == '_' || first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

macro_rules! contract_status {
    (
        $name:ident {
            $($variant:ident => $value:literal),+ $(,)?
        }
        terminal { $($terminal:ident),* $(,)? }
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value),+
                }
            }

            #[must_use]
            pub const fn is_terminal(self) -> bool {
                matches!(self, $(Self::$terminal)|*)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = &'static str;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    _ => Err(concat!("invalid ", stringify!($name))),
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

contract_status!(
    AgentProfileStatus {
        Active => "active",
        Disabled => "disabled",
        Retired => "retired",
    }
    terminal { Retired }
);

contract_status!(
    WorkerStatus {
        Online => "online",
        Draining => "draining",
        Offline => "offline",
        Retired => "retired",
    }
    terminal { Retired }
);

contract_status!(
    RuntimeSessionStatus {
        Requested => "requested",
        Starting => "starting",
        Running => "running",
        ResumePending => "resume_pending",
        Completed => "completed",
        Failed => "failed",
        Cancelled => "cancelled",
        Lost => "lost",
    }
    terminal { Completed, Failed, Cancelled, Lost }
);

contract_status!(
    RuntimeAttemptStatus {
        Starting => "starting",
        Running => "running",
        Exited => "exited",
        Gone => "gone",
    }
    terminal { Exited, Gone }
);

contract_status!(
    LeaseStatus {
        Active => "active",
        Released => "released",
        Expired => "expired",
        Cancelled => "cancelled",
        Superseded => "superseded",
    }
    terminal { Released, Expired, Cancelled, Superseded }
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct FencingToken(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("fencing token must be greater than zero")]
pub struct InvalidFencingToken;

impl FencingToken {
    /// Construct a non-zero task-scoped fencing token.
    ///
    /// # Errors
    /// Returns [`InvalidFencingToken`] when `value` is zero.
    pub const fn new(value: u64) -> Result<Self, InvalidFencingToken> {
        if value == 0 {
            Err(InvalidFencingToken)
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for FencingToken {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

impl fmt::Display for FencingToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskLeaseClaim {
    pub execution_task_id: TaskRunId,
    pub worker_incarnation_id: WorkerIncarnationId,
    pub lease_id: LeaseId,
    pub fencing_token: FencingToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskLeaseGrant {
    pub lease_id: LeaseId,
    pub execution_task_id: TaskRunId,
    pub worker_incarnation_id: WorkerIncarnationId,
    pub fencing_token: FencingToken,
    pub status: LeaseStatus,
    pub acquired_at: i64,
    pub expires_at: i64,
    pub renewed_at: Option<i64>,
    pub terminal_at: Option<i64>,
    pub terminal_reason: Option<String>,
    pub record_version: u64,
    #[serde(default)]
    pub execution_spec: Option<NativeExecutionSpec>,
    /// Control-plane-issued scope required by a remote native worker. Older
    /// durable leases may omit it and therefore cannot enter secured native
    /// execution until re-issued.
    #[serde(default)]
    pub security_scope: Option<ExecutionSecurityScope>,
    /// Durable logical session to which a native task is bound.
    #[serde(default)]
    pub runtime_session_id: Option<RuntimeSessionId>,
}

impl TaskLeaseGrant {
    #[must_use]
    pub fn claim(&self) -> TaskLeaseClaim {
        TaskLeaseClaim {
            execution_task_id: self.execution_task_id.clone(),
            worker_incarnation_id: self.worker_incarnation_id.clone(),
            lease_id: self.lease_id.clone(),
            fencing_token: self.fencing_token,
        }
    }
}
