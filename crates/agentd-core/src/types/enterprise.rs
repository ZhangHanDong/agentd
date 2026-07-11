//! Enterprise execution lifecycle states shared by storage and future APIs.

use std::fmt;

use serde::{Deserialize, Serialize};

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
