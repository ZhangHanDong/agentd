//! The agentd MCP tools (design §4.12.1), each a pure function over the
//! [`crate::host::RunHost`] seam. Task 1 lands `query_run` + `submit_outcome`;
//! `submit_review` / `assign_task` / `check_inbox` follow in 7a Tasks 2–3.

pub mod assign_task;
pub mod check_inbox;
pub mod query_run;
pub mod submit_human_answer;
pub mod submit_outcome;
pub mod submit_review;
