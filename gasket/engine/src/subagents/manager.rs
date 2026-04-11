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
}

impl TaskSpec {
    pub fn new(id: impl Into<String>, task: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            task: task.into(),
            model: None,
            system_prompt: None,
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
}

/// Spawn a subagent with minimal overhead - pure function.
///
/// This is the core function that replaces SubagentManager + SubagentTaskBuilder.
/// It takes everything needed as parameters and spawns a tokio task.
///
/// # Arguments
/// * `provider` - LLM provider
/// * `tools` - Tool registry
/// * `workspace` - Workspace path for loading system prompts
/// * `task` - Task specification
/// * `event_tx` - Optional channel for streaming events
/// * `result_tx` - Channel for the final result
/// * `token_tracker` - Optional shared token tracker for budget enforcement
///
/// # Returns
/// A `JoinHandle` for the spawned task.
#[instrument(name = "subagent.spawn", skip_all, fields(subagent_id = %task.id))]
pub fn spawn_subagent(
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    workspace: std::path::PathBuf,
    task: TaskSpec,
    event_tx: Option<mpsc::Sender<StreamEvent>>,
    result_tx: mpsc::Sender<SubagentResult>,
    token_tracker: Option<Arc<crate::token_tracker::TokenTracker>>,
) -> JoinHandle<()> {
    let subagent_id = task.id.clone();
    let task_desc = task.task.clone();
    let model = task.model.clone();

    tokio::spawn(async move {
        info!(
            "[Subagent {}] Starting task: {} (model: {:?})",
            subagent_id, task_desc, model
        );

        // Send started event
        if let Some(ref tx) = event_tx {
            let _ = tx.try_send(StreamEvent::subagent_started(&subagent_id, &task_desc, 1));
        }

        // Load system prompt
        let system_prompt = match task.system_prompt {
            Some(p) => p,
            None => match prompt::load_system_prompt(&workspace, prompt::BOOTSTRAP_FILES_MINIMAL)
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
            },
        };

        // Build kernel context
        let config = AgentConfig {
            model: model
                .clone()
                .unwrap_or_else(|| provider.default_model().to_string()),
            max_iterations: 10,
            ..Default::default()
        };
        let ctx = kernel::RuntimeContext {
            provider,
            tools,
            config: config.to_kernel_config(),
            spawner: None,
            token_tracker: token_tracker.clone(),
            pricing: None,
        };

        // Execute with timeout
        let timeout = std::time::Duration::from_secs(SUBAGENT_TIMEOUT_SECS);
        let messages = vec![
            ChatMessage::system(system_prompt),
            ChatMessage::user(&task_desc),
        ];

        let response = match tokio::time::timeout(
            timeout,
            execute_with_streaming(&ctx, messages, event_tx.as_ref(), &subagent_id),
        )
        .await
        {
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
        };

        // Accumulate token usage
        if let Some(ref tracker) = token_tracker {
            if let Some(ref usage) = response.token_usage {
                let token_usage =
                    gasket_types::TokenUsage::new(usage.input_tokens, usage.output_tokens);
                tracker.accumulate(&token_usage, response.cost);
            }
        }

        // Send completion event
        if let Some(ref tx) = event_tx {
            let summary = response.content.chars().take(100).collect::<String>();
            let _ = tx.try_send(StreamEvent::subagent_completed(
                &subagent_id,
                1,
                summary,
                response.tools_used.len() as u32,
            ));
        }

        // Send result
        let result = SubagentResult {
            id: subagent_id.clone(),
            task: task_desc,
            response: AgentResponse {
                content: response.content,
                reasoning_content: response.reasoning_content,
                tools_used: response.tools_used,
                model: model.clone(),
                token_usage: response.token_usage,
                cost: response.cost,
            },
            model,
        };

        if let Err(e) = result_tx.send(result).await {
            warn!("[Subagent {}] Failed to send result: {}", subagent_id, e);
        }
    })
}

/// Execute kernel with streaming support, forwarding events with subagent_id.
async fn execute_with_streaming(
    ctx: &kernel::RuntimeContext,
    messages: Vec<ChatMessage>,
    event_tx: Option<&mpsc::Sender<StreamEvent>>,
    subagent_id: &str,
) -> anyhow::Result<kernel::ExecutionResult> {
    let (kernel_tx, mut kernel_rx) = mpsc::channel(64);

    // Spawn kernel execution
    let ctx_clone = ctx.clone();
    let handle =
        tokio::spawn(
            async move { kernel::execute_streaming(&ctx_clone, messages, kernel_tx).await },
        );

    // Forward events with subagent_id
    if let Some(tx) = event_tx {
        while let Some(event) = kernel_rx.recv().await {
            // Convert kernel event to subagent event by injecting agent_id
            let subagent_event = inject_agent_id(event, subagent_id);
            let _ = tx.try_send(subagent_event);
        }
    }

    // Wait for completion
    match handle.await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(e)) => Err(anyhow::anyhow!("Kernel error: {}", e)),
        Err(e) => Err(anyhow::anyhow!("Task join error: {}", e)),
    }
}

/// Inject subagent_id into a StreamEvent.
///
/// This transforms a main agent event into a subagent event by setting agent_id.
fn inject_agent_id(event: StreamEvent, subagent_id: &str) -> StreamEvent {
    match event {
        StreamEvent::Thinking {
            agent_id: _,
            content,
        } => StreamEvent::Thinking {
            agent_id: Some(subagent_id.to_string()),
            content,
        },
        StreamEvent::ToolStart {
            agent_id: _,
            name,
            arguments,
        } => StreamEvent::ToolStart {
            agent_id: Some(subagent_id.to_string()),
            name,
            arguments,
        },
        StreamEvent::ToolEnd {
            agent_id: _,
            name,
            output,
        } => StreamEvent::ToolEnd {
            agent_id: Some(subagent_id.to_string()),
            name,
            output,
        },
        StreamEvent::Content {
            agent_id: _,
            content,
        } => StreamEvent::Content {
            agent_id: Some(subagent_id.to_string()),
            content,
        },
        StreamEvent::Done { agent_id: _ } => StreamEvent::Done {
            agent_id: Some(subagent_id.to_string()),
        },
        // TokenStats and lifecycle events are passed through as-is
        _ => event,
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

        // Resolve model if specified
        let (provider, model) = if let Some(ref mid) = model_id {
            if let Some(ref resolver) = self.model_resolver {
                resolver
                    .resolve_model(mid)
                    .map(|(p, c)| (p, Some(c.model)))
                    .unwrap_or((self.provider.clone(), None))
            } else {
                (self.provider.clone(), None)
            }
        } else {
            (self.provider.clone(), None)
        };

        let task_spec = TaskSpec::new(&subagent_id, task);
        let task_spec = if let Some(m) = model {
            task_spec.with_model(m)
        } else {
            task_spec
        };

        spawn_subagent(
            provider,
            self.tools.clone(),
            self.workspace.clone(),
            task_spec,
            Some(event_tx),
            result_tx,
            self.token_tracker.clone(),
        );

        let results = tracker
            .wait_for_all(1)
            .await
            .map_err(|e| anyhow::anyhow!("Tracker error: {}", e))?;

        if results.is_empty() {
            return Err(anyhow::anyhow!("Subagent completed but no result received").into());
        }

        let result = results.into_iter().next().unwrap();

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
}
