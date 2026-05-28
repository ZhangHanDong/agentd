//! See design §2.3.
//!
//! NOTE (per Engine Execution Model D2): `Status` has exactly four variants.
//! There is NO `Pending`. "Parked" is an engine control-flow state expressed by
//! `HandlerStep::Park` (see `engine/step.rs`), not a node Outcome.

use serde::{Deserialize, Serialize};

use crate::types::ids::NodeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Success,
    Fail,
    Retry,
    PartialSuccess,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_label: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggested_next_ids: Vec<NodeId>,
    #[serde(default)]
    pub context_updates: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mempal_writes: Vec<MempalWrite>,
}

impl Outcome {
    #[must_use]
    pub fn success() -> Self {
        Self {
            status: Status::Success,
            preferred_label: None,
            suggested_next_ids: Vec::new(),
            context_updates: serde_json::Map::new(),
            artifacts: Vec::new(),
            mempal_writes: Vec::new(),
        }
    }

    #[must_use]
    pub fn fail() -> Self {
        Self {
            status: Status::Fail,
            ..Self::success()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub kind: ArtifactKind,
    pub path: std::path::PathBuf,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    Spec,
    Plan,
    Diff,
    Transcript,
    Verdict,
    ContextPack,
}

// NOTE: the internal tag key is `op`, NOT `kind` — the `Ingest` variant has a
// field literally named `kind` (matching mempal's `mempal_ingest` API per design
// §4.12.2), which would collide with a `tag = "kind"` discriminant. Caught at
// P0.1 Task 1 build; the design/plan sample used `tag = "kind"` and did not compile.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MempalWrite {
    Ingest {
        wing: String,
        kind: String,
        body: String,
        importance: u8,
    },
    KgAdd {
        subject: String,
        predicate: String,
        object: String,
    },
    FactCheck {
        text: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regression test for the `tag = "kind"` collision fixed at P0.1 Task 1:
    // the Ingest variant's `kind` field must not clash with the discriminant key.
    #[test]
    fn mempal_write_round_trips_all_variants() {
        let cases = vec![
            MempalWrite::Ingest {
                wing: "proj".into(),
                kind: "spec".into(),
                body: "b".into(),
                importance: 5,
            },
            MempalWrite::KgAdd {
                subject: "s".into(),
                predicate: "p".into(),
                object: "o".into(),
            },
            MempalWrite::FactCheck { text: "t".into() },
        ];
        for w in cases {
            let json = serde_json::to_string(&w).expect("serialize");
            // the discriminant is keyed on "op", and Ingest still carries "kind"
            if let MempalWrite::Ingest { .. } = w {
                assert!(json.contains("\"op\":\"ingest\""), "missing op tag: {json}");
                assert!(
                    json.contains("\"kind\":\"spec\""),
                    "missing kind field: {json}"
                );
            }
            let back: MempalWrite = serde_json::from_str(&json).expect("deserialize");
            // round-trip preserves the JSON shape
            assert_eq!(serde_json::to_string(&back).expect("reserialize"), json);
        }
    }

    #[test]
    fn outcome_success_and_fail_round_trip() {
        for o in [Outcome::success(), Outcome::fail()] {
            let json = serde_json::to_string(&o).expect("serialize");
            let back: Outcome = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back.status, o.status);
        }
        // The Option/Vec fields use skip_serializing_if so they vanish when empty;
        // context_updates uses only #[serde(default)] (no skip), so an empty map is
        // always emitted as the context-patch slot. Pin that exact wire shape.
        let json = serde_json::to_string(&Outcome::success()).expect("serialize");
        assert_eq!(json, r#"{"status":"success","context_updates":{}}"#);
    }
}
