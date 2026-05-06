//! Pure-function kernel: the LLM execution loop with zero side effects.
//!
//! The kernel knows nothing about sessions, persistence, hooks, or memory.
//! It takes messages, calls the LLM, dispatches tools, and returns a result.

pub mod context;
pub mod error;
pub mod executor;
pub(crate) mod kernel_executor;
pub(crate) mod request_handler;
pub(crate) mod steppable_executor;
pub mod stream;
pub(crate) mod synthesis;
pub(crate) mod tool_executor;

pub use context::{CheckpointCallback, KernelConfig, RuntimeContext};
pub use error::KernelError;
pub use executor::{
    ExecutionResult, ExecutorOptions, KernelExecutor, StepResult, SteppableExecutor, TokenLedger,
    ToolExecutor,
};
pub use stream::{BufferedEvents, StreamEvent};

use gasket_providers::ChatMessage;
use tokio::sync::mpsc;
use tracing::debug;

/// Build a KernelExecutor from RuntimeContext.
fn build_executor(ctx: &RuntimeContext) -> KernelExecutor {
    debug!(
        "Building executor: model={}, max_iter={}, thinking={}",
        ctx.config.model, ctx.config.max_iterations, ctx.config.thinking_enabled
    );
    KernelExecutor::new(ctx.clone())
}

/// Pure function: streaming LLM conversation loop.
pub async fn execute_streaming(
    ctx: &RuntimeContext,
    messages: Vec<ChatMessage>,
    event_tx: mpsc::Sender<StreamEvent>,
) -> Result<ExecutionResult, KernelError> {
    let exec = build_executor(ctx);
    exec.execute(messages, event_tx, &ExecutorOptions::new()).await
}
