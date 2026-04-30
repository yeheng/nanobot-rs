//! Subagent spawning - simplified pure function approach
//!
//! This module replaces the 950-line Java-style Manager + Builder pattern
//! with a simple `spawn_subagent` function. KISS.
//!
//! ## Before (Java-style):
//! ```ignore
//! let manager = SubagentManager::new(...).await;
//! manager.task("id", "task").with_streaming(tx).spawn(result_tx).await?;
//! ```
//!
//! ## After (Rust-style):
//! ```ignore
//! let handle = spawn_subagent(ctx, task_spec, event_tx, result_tx);
//! ```

use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, instrument, warn};

use crate::kernel;
use crate::session::config::{AgentConfig, AgentConfigExt};
use crate::session::prompt;
use crate::tools::ToolRegistry;
use gasket_providers::{ChatMessage, LlmProvider};
use gasket_types::StreamEvent;

use super::runner::ModelResolver;
use super::tracker::SubagentResult;
use crate::session::AgentResponse;

/// Default timeout for subagent execution (10 minutes)
const SUBAGENT_TIMEOUT_SECS: u64 = 600;

/// Task specification for spawning a subagent
#[derive(Debug, Clone)]
pub struct TaskSpec {
    pub id: String,
    pub task: String,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub max_turns: Option<u32>,
}

impl TaskSpec {
    pub fn new(id: impl Into<String>, task: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            task: task.into(),
            model: None,
            system_prompt: None,
            max_turns: None,
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn with_max_turns(mut self, turns: u32) -> Self {
        self.max_turns = Some(turns);
        self
    }
}

/// Spawn a subagent with minimal overhead - pure function.
///
/// # Arguments
/// * `provider` - LLM provider
/// * `tools` - Tool registry
/// * `workspace` - Workspace path for loading system prompts
/// * `task` - Task specification
/// * `event_tx` - Optional channel for streaming events
/// * `result_tx` - Channel for the final result
/// * `token_tracker` - Optional shared token tracker for budget enforcement
/// * `cancellation_token` - Token to cancel this subagent (checked before and during execution)
///
/// # Returns
/// A `JoinHandle` for the spawned task.
#[instrument(name = "subagent.spawn", skip_all, fields(subagent_id = %task.id))]
#[allow(clippy::too_many_arguments)]
pub fn spawn_subagent(
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    workspace: std::path::PathBuf,
    task: TaskSpec,
    event_tx: Option<mpsc::Sender<StreamEvent>>,
    result_tx: mpsc::Sender<SubagentResult>,
    token_tracker: Option<Arc<crate::token_tracker::TokenTracker>>,
    cancellation_token: CancellationToken,
) -> JoinHandle<()> {
    let subagent_id = task.id.clone();
    let task_desc = task.task.clone();
    let model = task.model.clone();

    tokio::spawn(async move {
        info!(
            "[Subagent {}] Starting task: {} (model: {:?})",
            subagent_id, task_desc, model
        );

        // Load system prompt
        let system_prompt = match task.system_prompt {
            Some(p) => p,
            None => {
                match prompt::load_system_prompt(&workspace, prompt::BOOTSTRAP_FILES_MINIMAL, None)
                    .await
                {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(
                            "[Subagent {}] Failed to load system prompt: {}",
                            subagent_id, e
                        );
                        send_error_result(
                            &subagent_id,
                            &task_desc,
                            &model,
                            &format!("Prompt load failed: {}", e),
                            &result_tx,
                        )
                        .await;
                        return;
                    }
                }
            }
        };

        // Build kernel context
        let config = AgentConfig {
            model: model
                .clone()
                .unwrap_or_else(|| provider.default_model().to_string()),
            max_iterations: crate::session::config::DEFAULT_MAX_ITERATIONS,
            ..Default::default()
        };
        let ctx = kernel::RuntimeContext {
            provider,
            tools,
            config: config.to_kernel_config(),
            spawner: None,
            token_tracker: token_tracker.clone(),
            checkpoint_callback: None,
            session_key: None,
            outbound_tx: None,
            aggregator_cancel: None,
        };

        // Execute with timeout, cancellable via token
        let timeout = std::time::Duration::from_secs(SUBAGENT_TIMEOUT_SECS);
        let messages = vec![
            ChatMessage::system(system_prompt),
            ChatMessage::user(&task_desc),
        ];

        let response = tokio::select! {
            result = tokio::time::timeout(
                timeout,
                execute_with_streaming(&ctx, messages, event_tx.as_ref(), &subagent_id),
            ) => match result {
                Ok(Ok(resp)) => resp,
                Ok(Err(e)) => {
                    warn!("[Subagent {}] Execution failed: {}", subagent_id, e);
                    send_error_result(&subagent_id, &task_desc, &model, &e.to_string(), &result_tx)
                        .await;
                    return;
                }
                Err(_) => {
                    warn!("[Subagent {}] Timed out after {:?}", subagent_id, timeout);
                    send_error_result(
                        &subagent_id,
                        &task_desc,
                        &model,
                        &format!("Timed out after {}s", SUBAGENT_TIMEOUT_SECS),
                        &result_tx,
                    )
                    .await;
                    return;
                }
            },
            _ = cancellation_token.cancelled() => {
                info!("[Subagent {}] Cancelled", subagent_id);
                send_error_result(&subagent_id, &task_desc, &model, "Cancelled", &result_tx).await;
                return;
            }
        };

        // Accumulate token usage
        if let Some(ref tracker) = token_tracker {
            if let Some(ref usage) = response.token_usage {
                let token_usage =
                    gasket_types::TokenUsage::new(usage.input_tokens, usage.output_tokens);
                tracker.accumulate(&token_usage, 0.0);
            }
        }

        tracing::info!(
            subagent_id = %subagent_id,
            tool_count = response.tools_used.len(),
            "[Subagent] Completed: {}",
            response.content.chars().take(100).collect::<String>()
        );

        // Send result
        let result = SubagentResult {
            id: subagent_id.clone(),
            task: task_desc,
            response: AgentResponse::from_execution(response, model.clone()),
            model,
        };

        if let Err(e) = result_tx.send(result).await {
            warn!("[Subagent {}] Failed to send result: {}", subagent_id, e);
        }
    })
}

/// Execute kernel with streaming support, forwarding events tagged with subagent_id.
///
/// The `Arc<str>` for agent_id is allocated once before the loop; per-event cost
/// is a single `Arc::clone` (atomic refcount bump, no heap allocation).
///
/// When `event_tx` is `None`, the channel is still drained to prevent the kernel
/// task from blocking on a full channel (deadlock).
async fn execute_with_streaming(
    ctx: &kernel::RuntimeContext,
    messages: Vec<ChatMessage>,
    event_tx: Option<&mpsc::Sender<StreamEvent>>,
    subagent_id: &str,
) -> anyhow::Result<kernel::ExecutionResult> {
    let (kernel_tx, mut kernel_rx) = mpsc::channel(64);

    let ctx_clone = ctx.clone();
    let handle =
        tokio::spawn(
            async move { kernel::execute_streaming(&ctx_clone, messages, kernel_tx).await },
        );

    if let Some(tx) = event_tx {
        let agent_id: Arc<str> = Arc::from(subagent_id);
        while let Some(event) = kernel_rx.recv().await {
            if tx
                .try_send(event.with_agent_id(Arc::clone(&agent_id)))
                .is_err()
            {
                tracing::debug!(
                    "[Subagent {}] Client channel closed, draining kernel events...",
                    subagent_id
                );
                // Drain remaining events to unblock the kernel task.
                while kernel_rx.recv().await.is_some() {}
                break;
            }
        }
    } else {
        // Must drain to unblock the kernel task; otherwise the channel fills and deadlocks.
        while kernel_rx.recv().await.is_some() {}
    }

    match handle.await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(e)) => Err(anyhow::anyhow!("Kernel error: {}", e)),
        Err(e) => Err(anyhow::anyhow!("Task join error: {}", e)),
    }
}

/// Send an error result to the result channel.
async fn send_error_result(
    id: &str,
    task: &str,
    model: &Option<String>,
    error: &str,
    result_tx: &mpsc::Sender<SubagentResult>,
) {
    let result = SubagentResult {
        id: id.to_string(),
        task: task.to_string(),
        response: AgentResponse {
            content: format!("Error: {}", error),
            reasoning_content: None,
            tools_used: vec![],
            model: model.clone(),
            token_usage: None,
            cost: 0.0,
        },
        model: model.clone(),
    };
    let _ = result_tx.send(result).await;
}

// ============================================================================
// SubagentSpawner trait implementation
// ============================================================================

use async_trait::async_trait;
use gasket_types::{SubagentResponse, SubagentResult as TypesSubagentResult, SubagentSpawner};

/// Simple spawner implementation that holds the necessary context.
///
/// This replaces the 950-line SubagentManager with a simple struct
/// that implements SubagentSpawner.
#[derive(Clone)]
pub struct SimpleSpawner {
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    workspace: std::path::PathBuf,
    token_tracker: Option<Arc<crate::token_tracker::TokenTracker>>,
    model_resolver: Option<Arc<dyn ModelResolver>>,
}

impl SimpleSpawner {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        tools: Arc<ToolRegistry>,
        workspace: std::path::PathBuf,
    ) -> Self {
        Self {
            provider,
            tools,
            workspace,
            token_tracker: None,
            model_resolver: None,
        }
    }

    pub fn with_token_tracker(mut self, tracker: Arc<crate::token_tracker::TokenTracker>) -> Self {
        self.token_tracker = Some(tracker);
        self
    }

    pub fn with_model_resolver(mut self, resolver: Arc<dyn ModelResolver>) -> Self {
        self.model_resolver = Some(resolver);
        self
    }
}

#[async_trait]
impl SubagentSpawner for SimpleSpawner {
    async fn spawn(
        &self,
        task: String,
        model_id: Option<String>,
    ) -> Result<TypesSubagentResult, Box<dyn std::error::Error + Send>> {
        use super::tracker::SubagentTracker;

        let mut tracker = SubagentTracker::new();
        let result_tx = tracker.result_sender();
        let event_tx = tracker.event_sender();
        let subagent_id = SubagentTracker::generate_id();

        // Resolve provider and model: only attempt resolution when both model_id and resolver exist.
        let (provider, model) = match (&model_id, &self.model_resolver) {
            (Some(mid), Some(resolver)) => resolver
                .resolve_model(mid)
                .map(|(p, c)| (p, Some(c.model)))
                .unwrap_or((self.provider.clone(), None)),
            _ => (self.provider.clone(), None),
        };

        let task_spec = TaskSpec::new(&subagent_id, task);
        let task_spec = if let Some(m) = model {
            task_spec.with_model(m)
        } else {
            task_spec
        };
        let _task_desc = task_spec.task.clone();

        spawn_subagent(
            provider,
            self.tools.clone(),
            self.workspace.clone(),
            task_spec,
            Some(event_tx),
            result_tx,
            self.token_tracker.clone(),
            tracker.cancellation_token(),
        );

        let result = tracker
            .wait_for_all(1)
            .await
            .map_err(|e| anyhow::anyhow!("Tracker error: {}", e))?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("Subagent completed but no result received"))?;

        Ok(TypesSubagentResult {
            id: result.id,
            task: result.task,
            response: SubagentResponse {
                content: result.response.content,
                reasoning_content: result.response.reasoning_content,
                tools_used: result.response.tools_used,
                model: result.response.model,
                token_usage: result
                    .response
                    .token_usage
                    .map(|t| gasket_types::tool::TokenUsage {
                        prompt_tokens: t.input_tokens as u32,
                        completion_tokens: t.output_tokens as u32,
                        total_tokens: t.total_tokens as u32,
                    }),
                cost: result.response.cost,
            },
            model: result.model,
        })
    }

    async fn spawn_with_stream(
        &self,
        task: String,
        model_id: Option<String>,
    ) -> Result<
        (
            String,
            mpsc::Receiver<StreamEvent>,
            tokio::sync::oneshot::Receiver<TypesSubagentResult>,
            tokio_util::sync::CancellationToken,
        ),
        Box<dyn std::error::Error + Send>,
    > {
        use super::tracker::SubagentTracker;

        let mut tracker = SubagentTracker::new();
        let result_tx = tracker.result_sender();
        let event_tx = tracker.event_sender();
        let subagent_id = SubagentTracker::generate_id();

        // Resolve provider and model: only attempt resolution when both model_id and resolver exist.
        let (provider, model) = match (&model_id, &self.model_resolver) {
            (Some(mid), Some(resolver)) => resolver
                .resolve_model(mid)
                .map(|(p, c)| (p, Some(c.model)))
                .unwrap_or((self.provider.clone(), None)),
            _ => (self.provider.clone(), None),
        };

        let task_spec = TaskSpec::new(&subagent_id, task);
        let task_spec = if let Some(m) = model {
            task_spec.with_model(m)
        } else {
            task_spec
        };
        let task_desc = task_spec.task.clone();

        spawn_subagent(
            provider,
            self.tools.clone(),
            self.workspace.clone(),
            task_spec,
            Some(event_tx),
            result_tx,
            self.token_tracker.clone(),
            tracker.cancellation_token(),
        );

        let event_rx = tracker
            .take_event_receiver()
            .map_err(|e| anyhow::anyhow!("Failed to take event receiver: {}", e))?;

        let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
        let result_subagent_id = subagent_id.clone();
        let cancel_token = tracker.cancellation_token();

        tokio::spawn(async move {
            let types_result = match tracker.wait_for_all(1).await {
                Ok(results) => match results.into_iter().next() {
                    Some(result) => TypesSubagentResult {
                        id: result.id,
                        task: result.task,
                        response: SubagentResponse {
                            content: result.response.content,
                            reasoning_content: result.response.reasoning_content,
                            tools_used: result.response.tools_used,
                            model: result.response.model,
                            token_usage: result.response.token_usage.map(|t| {
                                gasket_types::tool::TokenUsage {
                                    prompt_tokens: t.input_tokens as u32,
                                    completion_tokens: t.output_tokens as u32,
                                    total_tokens: t.total_tokens as u32,
                                }
                            }),
                            cost: result.response.cost,
                        },
                        model: result.model,
                    },
                    None => TypesSubagentResult {
                        id: result_subagent_id.clone(),
                        task: task_desc.clone(),
                        response: SubagentResponse {
                            content: "Error: Subagent completed but no result received".to_string(),
                            reasoning_content: None,
                            tools_used: vec![],
                            model: None,
                            token_usage: None,
                            cost: 0.0,
                        },
                        model: None,
                    },
                },
                Err(e) => TypesSubagentResult {
                    id: result_subagent_id,
                    task: task_desc,
                    response: SubagentResponse {
                        content: format!("Error: {}", e),
                        reasoning_content: None,
                        tools_used: vec![],
                        model: None,
                        token_usage: None,
                        cost: 0.0,
                    },
                    model: None,
                },
            };
            let _ = completion_tx.send(types_result);
        });

        Ok((subagent_id, event_rx, completion_rx, cancel_token))
    }
}
