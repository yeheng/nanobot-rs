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
    pub thinking_enabled: bool,
    /// Optional whitelist of tool names visible to the LLM for this run.
    pub tool_filter: Option<Vec<String>>,
}

impl TaskSpec {
    pub fn new(id: impl Into<String>, task: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            task: task.into(),
            model: None,
            system_prompt: None,
            max_turns: None,
            thinking_enabled: false,
            tool_filter: None,
        }
    }

    pub fn with_thinking_enabled(mut self, enabled: bool) -> Self {
        self.thinking_enabled = enabled;
        self
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

    pub fn with_tool_filter(mut self, filter: Option<Vec<String>>) -> Self {
        self.tool_filter = filter;
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
/// * `cancellation_token` - Token to cancel this subagent (checked before and during execution)
/// * `refs` - Session-level references (token tracker, pending asks, session key, outbound channel)
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
    cancellation_token: CancellationToken,
    refs: gasket_types::SessionRefs,
    timeout_secs: u64,
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

        // Build kernel context. `tool_filter` is per-execution and lives on
        // KernelConfig, not AgentConfig — set it after `to_kernel_config()`.
        let agent_config = AgentConfig {
            model: model
                .clone()
                .unwrap_or_else(|| provider.default_model().to_string()),
            max_iterations: crate::session::config::DEFAULT_MAX_ITERATIONS,
            thinking_enabled: task.thinking_enabled,
            ..Default::default()
        };
        let mut kernel_config = agent_config.to_kernel_config();
        kernel_config.tool_filter = task.tool_filter.clone();
        let ctx = {
            let mut c =
                kernel::RuntimeContext::new_worker(provider, tools, kernel_config);
            c.refs = refs.clone();
            c
        };

        // Execute with timeout, cancellable via token
        let timeout = std::time::Duration::from_secs(timeout_secs);
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
                        &format!("Timed out after {}s", timeout_secs),
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
        if let Some(ref tracker) = ctx.refs.token_tracker {
            if let Some(ref usage) = response.token_usage {
                tracker.accumulate(usage, 0.0);
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
            match tx.send(event.with_agent_id(Arc::clone(&agent_id))).await {
                Ok(()) => {}
                Err(_) => {
                    tracing::debug!(
                        "[Subagent {}] Event channel closed, draining kernel events...",
                        subagent_id
                    );
                    // Drain remaining events to unblock the kernel task.
                    while kernel_rx.recv().await.is_some() {}
                    break;
                }
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
    /// Worker-flavour tool registry, pre-built without `spawn` / `spawn_parallel`.
    worker_tools: Arc<ToolRegistry>,
    workspace: std::path::PathBuf,
    budget: gasket_types::SpawnBudget,
    token_tracker: Option<Arc<crate::token_tracker::TokenTracker>>,
    model_resolver: Option<Arc<dyn ModelResolver>>,
    thinking_enabled: bool,
    pending_asks: Option<gasket_types::pending_ask::DynPendingAskRegistry>,
    timeout_secs: u64,
}

impl SimpleSpawner {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        worker_tools: Arc<ToolRegistry>,
        workspace: std::path::PathBuf,
        budget: gasket_types::SpawnBudget,
    ) -> Self {
        Self {
            provider,
            worker_tools,
            workspace,
            budget,
            token_tracker: None,
            model_resolver: None,
            thinking_enabled: false,
            pending_asks: None,
            timeout_secs: SUBAGENT_TIMEOUT_SECS,
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

    pub fn with_thinking_enabled(mut self, enabled: bool) -> Self {
        self.thinking_enabled = enabled;
        self
    }

    pub fn with_pending_asks(
        mut self,
        registry: gasket_types::pending_ask::DynPendingAskRegistry,
    ) -> Self {
        self.pending_asks = Some(registry);
        self
    }

    pub fn with_timeout_secs(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Build `SessionRefs` for a subagent worker context.
    ///
    /// Forwards the session-level refs that subagents need:
    /// token tracking, pending-ask routing, session identity, and outbound events.
    /// Fields intentionally not forwarded (spawner, synthesis_callback, aggregator_cancel)
    /// are either not applicable to workers or would violate the single-parent invariant.
    fn refs_for_subagent(
        &self,
        ctx: &gasket_types::tool::ToolContext,
    ) -> gasket_types::SessionRefs {
        gasket_types::SessionRefs {
            token_tracker: self.token_tracker.clone(),
            pending_asks: self.pending_asks.clone(),
            session_key: Some(ctx.session_key.clone()),
            outbound_tx: Some(ctx.outbound_tx.clone()),
            spawner: None,
            aggregator_cancel: None,
            synthesis_callback: None,
        }
    }

    /// Resolve provider and model from an optional model_id.
    ///
    /// When both `model_id` and `model_resolver` are present, attempts resolution.
    /// Falls back to the default provider with no model override on failure or absence.
    fn resolve_provider_model(
        &self,
        model_id: Option<&str>,
    ) -> (Arc<dyn LlmProvider>, Option<String>) {
        match (model_id, &self.model_resolver) {
            (Some(mid), Some(resolver)) => resolver
                .resolve_model(mid)
                .map(|(p, c)| (p, Some(c.model)))
                .unwrap_or_else(|| (self.provider.clone(), None)),
            _ => (self.provider.clone(), None),
        }
    }

    fn error_result(id: &str, task: &str, message: &str) -> TypesSubagentResult {
        TypesSubagentResult {
            id: id.to_string(),
            task: task.to_string(),
            response: SubagentResponse {
                content: format!("Error: {}", message),
                reasoning_content: None,
                tools_used: vec![],
                model: None,
                token_usage: None,
                cost: 0.0,
            },
            model: None,
        }
    }
}

#[async_trait]
impl SubagentSpawner for SimpleSpawner {
    async fn spawn(
        &self,
        task: String,
        model_id: Option<String>,
        ctx: &gasket_types::tool::ToolContext,
    ) -> Result<TypesSubagentResult, Box<dyn std::error::Error + Send>> {
        use super::tracker::SubagentTracker;

        let permit = self.budget.acquire().await;

        let mut tracker = SubagentTracker::new();
        let result_tx = tracker.result_sender();
        let event_tx = tracker.event_sender();
        let subagent_id = SubagentTracker::generate_id();

        let (provider, model) = self.resolve_provider_model(model_id.as_deref());

        let task_spec =
            TaskSpec::new(&subagent_id, task).with_thinking_enabled(self.thinking_enabled);
        let task_spec = if let Some(m) = model {
            task_spec.with_model(m)
        } else {
            task_spec
        };

        let refs = self.refs_for_subagent(ctx);
        let join_handle = spawn_subagent(
            provider,
            self.worker_tools.clone(),
            self.workspace.clone(),
            task_spec,
            Some(event_tx),
            result_tx,
            tracker.cancellation_token(),
            refs,
            self.timeout_secs,
        );
        // Hold the permit until the worker's tokio task ends.
        tokio::spawn(async move {
            let _permit = permit; // RAII: drops on guard-task exit
            let _ = join_handle.await; // wait for worker
        });

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
            response: result.response.into(),
            model: result.model,
        })
    }

    async fn spawn_with_stream(
        &self,
        task: String,
        model_id: Option<String>,
        ctx: &gasket_types::tool::ToolContext,
        tool_filter: Option<Vec<String>>,
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

        let (provider, model) = self.resolve_provider_model(model_id.as_deref());

        let task_spec = TaskSpec::new(&subagent_id, task)
            .with_thinking_enabled(self.thinking_enabled)
            .with_tool_filter(tool_filter);
        let task_spec = if let Some(m) = model {
            task_spec.with_model(m)
        } else {
            task_spec
        };
        let task_desc = task_spec.task.clone();

        let event_rx = tracker
            .take_event_receiver()
            .map_err(|e| anyhow::anyhow!("Failed to take event receiver: {}", e))?;

        let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
        let result_subagent_id = subagent_id.clone();
        let cancel_token = tracker.cancellation_token();
        let cancel_token_for_spawn = cancel_token.clone();

        // Move budget acquisition into the background so the caller
        // (e.g. spawn_parallel) is not blocked when concurrency is limited.
        let budget = self.budget.clone();
        let worker_tools = self.worker_tools.clone();
        let workspace = self.workspace.clone();
        let timeout_secs = self.timeout_secs;

        let refs = self.refs_for_subagent(ctx);

        tokio::spawn(async move {
            let _permit = budget.acquire().await;

            let _join_handle = spawn_subagent(
                provider,
                worker_tools,
                workspace,
                task_spec,
                Some(event_tx),
                result_tx,
                cancel_token_for_spawn,
                refs,
                timeout_secs,
            );

            let types_result = match tracker.wait_for_all(1).await {
                Ok(results) => match results.into_iter().next() {
                    Some(result) => TypesSubagentResult {
                        id: result.id,
                        task: result.task,
                        response: result.response.into(),
                        model: result.model,
                    },
                    None => Self::error_result(
                        &result_subagent_id,
                        &task_desc,
                        "Subagent completed but no result received",
                    ),
                },
                Err(e) => Self::error_result(&result_subagent_id, &task_desc, &e.to_string()),
            };
            let _ = completion_tx.send(types_result);
        });

        Ok((subagent_id, event_rx, completion_rx, cancel_token))
    }
}

#[cfg(test)]
mod budget_tests {
    use gasket_types::SpawnBudget;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    /// Verifies that `SpawnBudget` with limit=1 serializes concurrent acquires.
    /// Co-located with SimpleSpawner to ensure the contract stays wired.
    #[tokio::test]
    async fn budget_gate_serializes_concurrent_acquires() {
        let budget = SpawnBudget::new(1);
        let inflight = std::sync::Arc::new(AtomicUsize::new(0));
        let peak = std::sync::Arc::new(AtomicUsize::new(0));
        let start = Instant::now();

        let mut tasks = vec![];
        for _ in 0..3 {
            let b = budget.clone();
            let inflight = inflight.clone();
            let peak = peak.clone();
            tasks.push(tokio::spawn(async move {
                let _permit = b.acquire().await;
                let cur = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(cur, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(100)).await;
                inflight.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        let elapsed = start.elapsed();

        assert_eq!(
            peak.load(Ordering::SeqCst),
            1,
            "budget=1 must enforce inflight==1"
        );
        assert!(
            elapsed >= Duration::from_millis(280),
            "3 sequential 100ms tasks should take ≥280ms; took {:?}",
            elapsed
        );
    }
}
