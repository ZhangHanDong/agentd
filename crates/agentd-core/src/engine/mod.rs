//! Workflow execution engine: the control-flow vocabulary (`step`), checkpoint
//! persistence (`checkpoint`), goal-gate evaluation (`goal_gate`), and the run
//! loop + event delivery (`execute`).

pub mod checkpoint;
pub mod execute;
pub mod goal_gate;
pub mod step;

pub use checkpoint::Checkpoint;
pub use execute::Engine;
pub use goal_gate::GoalGateStatus;
pub use step::{EngineEvent, HandlerStep, ParkReason, RunProgress};
