//! Tools for interacting with the state machine.
//!
//! Provides:
//! - `StateMachineTaskTool`: Create, get, list, and transition tasks
//! - `ReportProgressTool`: Report progress from agents

mod report_progress;
mod state_machine_task;

pub use report_progress::ReportProgressTool;
pub use state_machine_task::StateMachineTaskTool;
