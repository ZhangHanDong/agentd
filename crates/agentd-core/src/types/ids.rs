//! Strongly-typed IDs. Newtype pattern — IDs of different domains never substitute.

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! id_newtype {
    ($name:ident, $prefix:literal) => {
        // Ord/PartialOrd enable use as keys in ordered maps (e.g. Checkpoint's
        // retry_counts: BTreeMap<NodeId, u32>) for deterministic JSON key order.
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(format!("{}_{}", $prefix, ulid::Ulid::new()))
            }

            pub fn from_string(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

id_newtype!(RunId, "r");
id_newtype!(TaskRunId, "tr");
id_newtype!(ReviewRunId, "rv");
id_newtype!(NodeId, "n"); // not auto-generated typically; parsed from DOT
id_newtype!(AgentId, "a"); // typically human-set ("claude-impl-a")
id_newtype!(AgentProfileId, "ap");
id_newtype!(WorkerId, "wk");
id_newtype!(WorkerIncarnationId, "wi");
id_newtype!(RuntimeSessionId, "rs");
id_newtype!(RuntimeAttemptId, "ra");
id_newtype!(ExecutionArtifactId, "ar");
id_newtype!(AuditEventId, "ae");
id_newtype!(LeaseId, "ls");
id_newtype!(FleetOutboxId, "fo");
id_newtype!(ArtifactUploadId, "au");
id_newtype!(MatrixCommandId, "mc");
id_newtype!(MatrixGatewayOutboxId, "mo");

impl NodeId {
    /// `NodeId`s in DOT files are operator-authored; preserve as-is.
    pub fn parsed(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

impl AgentId {
    /// Agent IDs are operator-authored kebab-case strings.
    pub fn parsed(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}
