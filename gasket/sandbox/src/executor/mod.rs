//! Command execution module
//!
//! Provides high-level command execution with sandbox support,
//! timeout handling, and output management.

mod command;
mod process;
mod result;

pub use command::CommandBuilder;
pub use process::ProcessManager;
pub use result::ExecutionResult;
