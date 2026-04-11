//! Pure-function kernel: the LLM execution loop with zero side effects.
//!
//! The kernel knows nothing about sessions, persistence, hooks, or memory.
//! It takes messages, calls the LLM, dispatches tools, and returns a result.

pub mod context;
pub mod error;
pub mod executor;
pub mod stream;

pub use context::{KernelConfig, RuntimeContext};
pub use error::KernelError;
pub use executor::{AgentExecutor, ExecutionResult, ExecutorOptions, ToolExecutor};
pub use stream::{BufferedEvents, StreamEvent};

use gasket_providers::ChatMessage;
use tokio::sync::mpsc;

/// Build an AgentExecutor and ExecutorOptions from RuntimeContext.
fn build_executor_and_options(ctx: &RuntimeContext) -> (AgentExecutor<'_>, ExecutorOptions<'_>) {
    let exec = AgentExecutor::new(ctx.provider.clone(), ctx.tools.clone(), &ctx.config);
    let mut options = ExecutorOptions::new();
    if let Some(ref tracker) = ctx.token_tracker {
        options = options.with_token_tracker(tracker.clone());
    }
    if let Some(ref pricing) = ctx.pricing {
        options = options.with_pricing(pricing.clone());
    }
    (exec, options)
}

/// Pure function: execute LLM conversation loop (non-streaming).
///
/// Internally delegates to the streaming kernel — events are silently drained.
pub async fn execute(
    ctx: &RuntimeContext,
    messages: Vec<ChatMessage>,
) -> Result<ExecutionResult, KernelError> {
    let (exec, options) = build_executor_and_options(ctx);
    exec.execute_with_options(messages, &options).await
}

/// Pure function: streaming LLM conversation loop.
pub async fn execute_streaming(
    ctx: &RuntimeContext,
    messages: Vec<ChatMessage>,
    event_tx: mpsc::Sender<StreamEvent>,
) -> Result<ExecutionResult, KernelError> {
    let (exec, options) = build_executor_and_options(ctx);
    exec.execute_stream_with_options(messages, event_tx, &options)
        .await
}
