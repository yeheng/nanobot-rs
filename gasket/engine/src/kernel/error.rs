//! Kernel-specific errors.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum KernelError {
    #[error("Provider request failed: {0}")]
    Provider(String),
    #[error("Max iterations ({0}) reached")]
    MaxIterations(u32),
    #[error("Tool execution failed: {0}")]
    ToolExecution(String),
}
