//! Workflow execution engine. P0.1 lands the control-flow vocabulary (`step`),
//! checkpoint persistence (`checkpoint`), and goal-gate evaluation (`goal_gate`);
//! the run loop arrives in Task 9.

pub mod checkpoint;
pub mod goal_gate;
pub mod step;

pub use checkpoint::Checkpoint;
pub use goal_gate::GoalGateStatus;
pub use step::{EngineEvent, HandlerStep, ParkReason, RunProgress};
