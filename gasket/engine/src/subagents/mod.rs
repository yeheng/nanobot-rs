pub mod manager;
pub mod runner;
pub mod tracker;

// Re-exports
pub use manager::SubagentManager;
pub use runner::{run_subagent, ModelResolver};
pub use tracker::{SubagentTracker, TrackerError};
