//! Kernel executor facade — re-exports from submodules.
//!
//! The implementation has been split into focused modules:
//! - `tool_executor`: Single tool call execution
//! - `request_handler`: LLM request building and retry logic
//! - `steppable_executor`: One-step LLM + tool execution
//! - `kernel_executor`: Full multi-turn execution loop

pub use crate::kernel::kernel_executor::{
    ExecutionResult, ExecutorOptions, KernelExecutor, TokenLedger,
};
pub use crate::kernel::request_handler::RequestHandler;
pub use crate::kernel::steppable_executor::{StepResult, SteppableExecutor};
pub use crate::kernel::tool_executor::{ToolCallResult, ToolExecutor};
