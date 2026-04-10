//! Subagent manager for background task execution
//!
//! This module provides a Builder pattern API for spawning subagent tasks.
//! The `SubagentTaskBuilder` consolidates all the scattered `submit_*` methods
//! into a single, fluent API.

use std::ops::Deref;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, instrument, warn};

use super::tracker::{SubagentEvent, SubagentResult};
use crate::agent::core::AgentConfig;
use crate::agent::execution::prompt;
use crate::bus::events::{OutboundMessage, SessionKey};
use crate::kernel;
use crate::session::config::AgentConfigExt;
use crate::tools::ToolRegistry;
use gasket_providers::{ChatMessage, LlmProvider};

use crate::agent::core::loop_::AgentResponse;

/// Default timeout for subagent execution (10 minutes)
const SUBAGENT_TIMEOUT_SECS: u64 = 600;

use super::runner::ModelResolver;

// ── Kernel helpers ──────────────────────────────────────────────────────────

/// Build a RuntimeContext for subagent execution.
fn build_kernel_context(
    provider: Arc<dyn LlmProvider>,
    config: &AgentConfig,
    tools: Arc<ToolRegistry>,
    token_tracker: Option<Arc<crate::token_tracker::TokenTracker>>,
) -> kernel::RuntimeContext {
    kernel::RuntimeContext {
        provider,
        tools,
        config: config.to_kernel_config(),
        spawner: None,
        token_tracker,
    }
}

/// Convert kernel ExecutionResult to AgentResponse.
fn to_agent_response(result: kernel::ExecutionResult, model: &str) -> AgentResponse {
    AgentResponse {
        content: result.content,
        reasoning_content: result.reasoning_content,
        tools_used: result.tools_used,
        model: Some(model.to_string()),
        token_usage: result.token_usage,
        cost: result.cost,
    }
}

/// Build messages for kernel execution from system prompt and user task.
fn build_kernel_messages(system_prompt: &str, task: &str) -> Vec<ChatMessage> {
    vec![
        ChatMessage::system(system_prompt),
        ChatMessage::user(task),
    ]
}

/// RAII guard for session key management.
///
/// Automatically clears the session key when dropped, ensuring
/// cleanup even if the request panics.
///
/// # Example
///
/// ```ignore
/// let manager = SubagentManager::new(...);
/// {
///     let _guard = manager.session_key_guard(session_key);
///     // Session key is set
///     manager.get_session_key(); // Some(session_key)
/// }
/// // Session key is automatically cleared
/// manager.get_session_key(); // None
/// ```
pub struct SessionKeyGuard<'a> {
    manager: &'a SubagentManager,
}

impl<'a> SessionKeyGuard<'a> {
    fn new(manager: &'a SubagentManager, session_key: SessionKey) -> Self {
        manager.set_session_key_internal(session_key);
        Self { manager }
    }
}

impl<'a> Deref for SessionKeyGuard<'a> {
    type Target = SubagentManager;

    fn deref(&self) -> &Self::Target {
        self.manager
    }
}

impl<'a> Drop for SessionKeyGuard<'a> {
    fn drop(&mut self) {
        self.manager.clear_session_key_internal();
    }
}

pub struct SubagentManager {
    provider: Arc<dyn LlmProvider>,
    workspace: PathBuf,
    tools: Arc<ToolRegistry>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    /// Session key for WebSocket streaming (set per-request).
    /// Uses Mutex instead of RwLock because access is serial (one request at a time).
    session_key: Arc<std::sync::Mutex<Option<SessionKey>>>,
    /// Subagent execution timeout in seconds
    timeout_secs: u64,
    /// Optional model resolver for switching models in subagents.
    model_resolver: Option<Arc<dyn ModelResolver>>,
    /// Token tracker shared across parent and subagents for budget enforcement.
    token_tracker: Option<Arc<crate::token_tracker::TokenTracker>>,
}

/// Builder for configuring and spawning subagent tasks.
///
/// This replaces the scattered `submit_*` methods with a single fluent API.
/// All optional parameters are configured via builder methods, then `spawn()`
/// executes the task.
///
/// # Example
///
/// ```ignore
/// let result = manager.task("my-task-id", "Analyze the codebase")
///     .with_provider(custom_provider)
///     .with_config(AgentConfig { model: "gpt-4".into(), ..Default::default() })
///     .with_streaming(event_tx)
///     .spawn(result_tx)
///     .await?;
/// ```
pub struct SubagentTaskBuilder<'a> {
    manager: &'a SubagentManager,
    /// Unique identifier for this subagent task
    subagent_id: String,
    /// The task prompt to execute
    task: String,
    /// Optional custom provider (uses manager's default if None)
    provider: Option<Arc<dyn LlmProvider>>,
    /// Optional agent configuration (uses default if None)
    agent_config: Option<AgentConfig>,
    /// Optional event channel for streaming updates
    event_tx: Option<mpsc::Sender<SubagentEvent>>,
    /// Optional custom system prompt (uses minimal bootstrap if None)
    system_prompt: Option<String>,
    /// Session key for WebSocket streaming (passed directly, not stored in manager)
    session_key: Option<SessionKey>,
    /// Cancellation token for graceful shutdown
    cancellation_token: Option<tokio_util::sync::CancellationToken>,
    /// Token tracker shared with parent for budget enforcement
    token_tracker: Option<Arc<crate::token_tracker::TokenTracker>>,
}

impl<'a> SubagentTaskBuilder<'a> {
    /// Create a new task builder with required parameters.
    pub fn new(manager: &'a SubagentManager, subagent_id: String, task: String) -> Self {
        Self {
            manager,
            subagent_id,
            task,
            provider: None,
            agent_config: None,
            event_tx: None,
            system_prompt: None,
            session_key: None,
            cancellation_token: None,
            token_tracker: None,
        }
    }

    /// Set a custom LLM provider for this task.
    pub fn with_provider(mut self, provider: Arc<dyn LlmProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Set custom agent configuration (model, temperature, etc.).
    pub fn with_config(mut self, config: AgentConfig) -> Self {
        self.agent_config = Some(config);
        self
    }

    /// Enable streaming events via the provided channel.
    pub fn with_streaming(mut self, event_tx: mpsc::Sender<SubagentEvent>) -> Self {
        self.event_tx = Some(event_tx);
        self
    }

    /// Set a custom system prompt (uses minimal bootstrap if not set).
    pub fn with_system_prompt(mut self, prompt: String) -> Self {
        self.system_prompt = Some(prompt);
        self
    }

    /// Set the session key for WebSocket streaming.
    ///
    /// This replaces the old pattern of storing session_key in SubagentManager
    /// with Arc<RwLock>. Now the session_key is passed directly with the task.
    pub fn with_session_key(mut self, session_key: SessionKey) -> Self {
        self.session_key = Some(session_key);
        self
    }

    /// Set the cancellation token for graceful shutdown.
    ///
    /// The subagent task will check this token periodically and stop
    /// execution when cancelled.
    pub fn with_cancellation_token(mut self, token: tokio_util::sync::CancellationToken) -> Self {
        self.cancellation_token = Some(token);
        self
    }

    /// Set the token tracker for budget enforcement across parent and subagents.
    ///
    /// When set, the subagent will accumulate its token usage to the shared tracker,
    /// enabling unified budget enforcement across all parallel spawns.
    pub fn with_token_tracker(mut self, tracker: Arc<crate::token_tracker::TokenTracker>) -> Self {
        self.token_tracker = Some(tracker);
        self
    }

    /// Resolve the provider for this subagent task.
    fn resolve_provider(&self) -> Arc<dyn LlmProvider> {
        self.provider
            .clone()
            .unwrap_or_else(|| self.manager.provider.clone())
    }

    /// Resolve or default the agent configuration.
    fn resolve_agent_config(&self, provider: &dyn LlmProvider) -> AgentConfig {
        self.agent_config.clone().unwrap_or_else(|| AgentConfig {
            model: provider.default_model().to_string(),
            max_iterations: 10,
            ..Default::default()
        })
    }

    /// Spawn the subagent task and return its ID.
    ///
    /// The task runs in the background and sends its result to `result_tx`
    /// when complete. If streaming is enabled, events are sent to `event_tx`.
    #[instrument(name = "subagent.spawn", skip_all)]
    pub async fn spawn(self, result_tx: mpsc::Sender<SubagentResult>) -> anyhow::Result<String> {
        // Resolve configuration
        let provider = self.resolve_provider();
        let agent_config = self.resolve_agent_config(provider.as_ref());
        let model_name = agent_config.model.clone();
        let workspace = self.manager.workspace.clone();
        let tools = self.manager.tools.clone();
        let task = self.task.clone();
        let subagent_id = self.subagent_id.clone();
        let cancellation_token = self.cancellation_token.clone();
        let event_tx = self.event_tx.clone();
        let system_prompt_override = self.system_prompt;
        let token_tracker = self.token_tracker.clone();

        // Spawn background task
        tokio::spawn(async move {
            info!(
                "[Subagent {}] Task started with model '{}': {}",
                &subagent_id, &model_name, &task
            );

            // Check cancellation at startup
            if let Some(ref token) = cancellation_token {
                if token.is_cancelled() {
                    warn!("[Subagent {}] Cancelled before starting", subagent_id);
                    return;
                }
            }

            // Send started event
            if let Some(ref tx) = event_tx {
                let _ = tx.try_send(SubagentEvent::Started {
                    id: subagent_id.clone(),
                    task: task.clone(),
                });
            }

            // Build kernel context
            let ctx =
                build_kernel_context(provider, &agent_config, tools, token_tracker.clone());

            // Load system prompt
            let system_prompt = match system_prompt_override {
                Some(p) => p,
                None => {
                    match prompt::load_system_prompt(&workspace, prompt::BOOTSTRAP_FILES_MINIMAL)
                        .await
                    {
                        Ok(p) => p,
                        Err(e) => {
                            warn!(
                                "[Subagent {}] Failed to load system prompt: {}",
                                subagent_id, e
                            );
                            Self::send_initialization_error(
                                &subagent_id,
                                &task,
                                &model_name,
                                &format!("System prompt load failed: {}", e),
                                &event_tx,
                                &result_tx,
                            )
                            .await;
                            return;
                        }
                    }
                }
            };

            // Execute with timeout via kernel
            let response = Self::execute_kernel_with_timeout(
                ctx,
                &system_prompt,
                &task,
                &event_tx,
                &subagent_id,
                &cancellation_token,
                &model_name,
            )
            .await;

            // Accumulate token usage to parent tracker if provided
            if let Some(ref tracker) = token_tracker {
                if let Ok(ref resp) = response {
                    if let Some(ref usage) = resp.token_usage {
                        let token_usage =
                            gasket_types::TokenUsage::new(usage.input_tokens, usage.output_tokens);
                        tracker.accumulate(&token_usage, resp.cost);
                        tracing::debug!(
                            "[Subagent {}] Accumulated {} tokens (cost: ${:.4}) to parent tracker",
                            subagent_id,
                            token_usage.total_tokens,
                            resp.cost
                        );
                    }
                }
            }

            // Dispatch result
            Self::dispatch_result(
                response,
                result_tx,
                &event_tx,
                &subagent_id,
                task,
                model_name,
            )
            .await;
        });

        Ok(self.subagent_id)
    }

    /// Send initialization error to both event and result channels.
    async fn send_initialization_error(
        subagent_id: &str,
        task: &str,
        model_name: &str,
        error: &str,
        event_tx: &Option<mpsc::Sender<SubagentEvent>>,
        result_tx: &mpsc::Sender<SubagentResult>,
    ) {
        if let Some(ref tx) = event_tx {
            let _ = tx.try_send(SubagentEvent::Error {
                id: subagent_id.to_string(),
                error: error.to_string(),
            });
        }

        let error_response = AgentResponse {
            content: format!("Error: {}", error),
            reasoning_content: None,
            tools_used: vec![],
            model: Some(model_name.to_string()),
            token_usage: None,
            cost: 0.0,
        };

        let _ = result_tx
            .send(SubagentResult {
                id: subagent_id.to_string(),
                task: task.to_string(),
                response: error_response,
                model: Some(model_name.to_string()),
            })
            .await;
    }

    /// Execute subagent via kernel with timeout and cancellation support.
    async fn execute_kernel_with_timeout(
        ctx: kernel::RuntimeContext,
        system_prompt: &str,
        task: &str,
        event_tx: &Option<mpsc::Sender<SubagentEvent>>,
        subagent_id: &str,
        cancellation_token: &Option<tokio_util::sync::CancellationToken>,
        model_name: &str,
    ) -> Result<AgentResponse, anyhow::Error> {
        let timeout_duration = std::time::Duration::from_secs(SUBAGENT_TIMEOUT_SECS);

        if let Some(tx) = event_tx {
            // Streaming path
            Self::execute_kernel_streaming(
                ctx,
                system_prompt,
                task,
                tx,
                subagent_id,
                cancellation_token,
                timeout_duration,
                model_name,
            )
            .await
        } else {
            // Non-streaming path
            let messages = build_kernel_messages(system_prompt, task);
            match tokio::time::timeout(timeout_duration, kernel::execute(&ctx, messages)).await {
                Ok(Ok(result)) => Ok(to_agent_response(result, model_name)),
                Ok(Err(e)) => Err(anyhow::anyhow!("Execution failed: {}", e)),
                Err(_) => Err(anyhow::anyhow!("Timed out after {:?}", timeout_duration)),
            }
        }
    }

    /// Execute subagent via kernel with streaming support.
    async fn execute_kernel_streaming(
        ctx: kernel::RuntimeContext,
        system_prompt: &str,
        task: &str,
        event_tx: &mpsc::Sender<SubagentEvent>,
        subagent_id: &str,
        cancellation_token: &Option<tokio_util::sync::CancellationToken>,
        timeout_duration: std::time::Duration,
        model_name: &str,
    ) -> Result<AgentResponse, anyhow::Error> {
        let tx_clone = event_tx.clone();
        let id_clone = subagent_id.to_string();
        let iteration_counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let cancellation_token_clone = cancellation_token.clone();

        let (kernel_event_tx, mut event_rx) = mpsc::channel(64);
        let ctx_clone = ctx.clone();
        let messages = build_kernel_messages(system_prompt, task);
        let model_name_owned = model_name.to_string();

        let result_handle = tokio::spawn(async move {
            let result = kernel::execute_streaming(&ctx_clone, messages, kernel_event_tx)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            Ok::<AgentResponse, anyhow::Error>(to_agent_response(result, &model_name_owned))
        });

        let forward_handle = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                match event {
                    kernel::StreamEvent::Content(content) => {
                        let _ = tx_clone.try_send(SubagentEvent::Content {
                            id: id_clone.clone(),
                            content,
                        });
                    }
                    kernel::StreamEvent::Reasoning(content) => {
                        let _ = tx_clone.try_send(SubagentEvent::Thinking {
                            id: id_clone.clone(),
                            content,
                        });
                    }
                    kernel::StreamEvent::ToolStart { name, arguments } => {
                        let _ = tx_clone.try_send(SubagentEvent::ToolStart {
                            id: id_clone.clone(),
                            tool_name: name,
                            arguments,
                        });
                    }
                    kernel::StreamEvent::ToolEnd { name, output } => {
                        let _ = tx_clone.try_send(SubagentEvent::ToolEnd {
                            id: id_clone.clone(),
                            tool_name: name,
                            output,
                        });
                    }
                    kernel::StreamEvent::Done => {
                        let iter = iteration_counter
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                            + 1;
                        let _ = tx_clone.try_send(SubagentEvent::Iteration {
                            id: id_clone.clone(),
                            iteration: iter,
                        });
                    }
                    kernel::StreamEvent::TokenStats { .. } => {}
                }
            }
        });

        let result = if let Some(token) = cancellation_token_clone {
            let token_clone = token.clone();
            tokio::select! {
                biased;
                _ = token_clone.cancelled() => {
                    warn!("[Subagent {}] Cancelled during execution", subagent_id);
                    Err(anyhow::anyhow!("cancelled"))
                }
                result = tokio::time::timeout(timeout_duration, result_handle) => {
                    let join_result = result.map_err(|_| anyhow::anyhow!("timed out"))?;
                    let inner: Result<AgentResponse, anyhow::Error> = join_result
                        .map_err(|e: tokio::task::JoinError| anyhow::anyhow!("Task join error: {}", e))?;
                    inner
                }
            }
        } else {
            let timeout_result = tokio::time::timeout(timeout_duration, result_handle)
                .await
                .map_err(|_| anyhow::anyhow!("timed out"))?;
            let inner: Result<AgentResponse, anyhow::Error> = timeout_result
                .map_err(|e: tokio::task::JoinError| anyhow::anyhow!("Task join error: {}", e))?;
            inner
        };

        let _ = forward_handle.await;

        result
    }

    /// Dispatch result to channels.
    async fn dispatch_result(
        response: Result<AgentResponse, anyhow::Error>,
        result_tx: mpsc::Sender<SubagentResult>,
        event_tx: &Option<mpsc::Sender<SubagentEvent>>,
        subagent_id: &str,
        task: String,
        model_name: String,
    ) {
        let response = match response {
            Ok(resp) => resp,
            Err(e) => {
                warn!("[Subagent {}] Execution failed: {}", subagent_id, e);
                AgentResponse {
                    content: format!("Error: {}", e),
                    reasoning_content: None,
                    tools_used: vec![],
                    model: Some(model_name.clone()),
                    token_usage: None,
                    cost: 0.0,
                }
            }
        };

        let subagent_result = SubagentResult {
            id: subagent_id.to_string(),
            task,
            response,
            model: Some(model_name),
        };

        if let Some(ref tx) = event_tx {
            let _ = tx.try_send(SubagentEvent::Completed {
                id: subagent_id.to_string(),
                result: subagent_result.clone(),
            });
        }

        if let Err(e) = result_tx.send(subagent_result).await {
            warn!("[Subagent {}] Failed to send result: {}", subagent_id, e);
        }
    }
}

/// Load system prompt for subagent execution.
async fn resolve_system_prompt(
    workspace: &std::path::Path,
    system_prompt: Option<&str>,
) -> anyhow::Result<String> {
    match system_prompt {
        Some(s) => Ok(s.to_string()),
        None => prompt::load_system_prompt(workspace, prompt::BOOTSTRAP_FILES_MINIMAL)
            .await
            .map_err(|e| anyhow::anyhow!("System prompt load failed: {}", e)),
    }
}

impl SubagentManager {
    pub async fn new(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        tools: Arc<ToolRegistry>,
        outbound_tx: mpsc::Sender<OutboundMessage>,
    ) -> Self {
        Self::with_model_resolver(provider, workspace, tools, outbound_tx, None).await
    }

    pub async fn with_model_resolver(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        tools: Arc<ToolRegistry>,
        outbound_tx: mpsc::Sender<OutboundMessage>,
        model_resolver: Option<Arc<dyn ModelResolver>>,
    ) -> Self {
        Self {
            provider,
            workspace,
            tools,
            outbound_tx,
            session_key: Arc::new(std::sync::Mutex::new(None)),
            timeout_secs: crate::agent::core::DEFAULT_SUBAGENT_TIMEOUT_SECS,
            model_resolver,
            token_tracker: None,
        }
    }

    /// Get the configured timeout in seconds.
    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }

    /// Get the timeout as a Duration.
    pub fn timeout_duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.timeout_secs)
    }

    /// Create an RAII guard for session key management.
    ///
    /// The session key will be automatically cleared when the guard is dropped,
    /// ensuring cleanup even if the request panics.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let manager = SubagentManager::new(...);
    /// {
    ///     let _guard = manager.session_key_guard(session_key);
    ///     // Use manager with session key set
    ///     manager.task("task-1", "Analyze code").spawn(result_tx).await?;
    /// }
    /// // Session key automatically cleared
    /// ```
    pub fn session_key_guard(&self, session_key: SessionKey) -> SessionKeyGuard<'_> {
        SessionKeyGuard::new(self, session_key)
    }

    /// Internal method to set session key (called by SessionKeyGuard)
    fn set_session_key_internal(&self, session_key: SessionKey) {
        let mut guard = self.session_key.lock().unwrap();
        *guard = Some(session_key);
    }

    /// Internal method to clear session key (called by SessionKeyGuard::drop)
    fn clear_session_key_internal(&self) {
        let mut guard = self.session_key.lock().unwrap();
        *guard = None;
    }

    /// Set the session key for the current request context.
    ///
    /// # Deprecation Notice
    ///
    /// Prefer using `session_key_guard()` for automatic cleanup.
    /// This method requires manual `clear_session_key()` call.
    #[deprecated(note = "Use session_key_guard() for automatic cleanup")]
    pub fn set_session_key(&self, session_key: SessionKey) {
        self.set_session_key_internal(session_key);
    }

    /// Clear the session key (call after request completes)
    ///
    /// # Deprecation Notice
    ///
    /// Prefer using `session_key_guard()` for automatic cleanup.
    #[deprecated(note = "Use session_key_guard() for automatic cleanup")]
    pub fn clear_session_key(&self) {
        self.clear_session_key_internal();
    }

    /// Get the current session key
    pub fn get_session_key(&self) -> Option<SessionKey> {
        self.session_key.lock().unwrap().clone()
    }

    /// Get a clone of the outbound sender for external use
    pub fn outbound_sender(&self) -> mpsc::Sender<OutboundMessage> {
        self.outbound_tx.clone()
    }

    /// Set the token tracker for budget enforcement across parent and subagents.
    pub fn with_token_tracker(mut self, tracker: Arc<crate::token_tracker::TokenTracker>) -> Self {
        self.token_tracker = Some(tracker);
        self
    }

    /// Create a task builder for fluent subagent configuration.
    ///
    /// This is the preferred way to spawn subagent tasks, replacing all
    /// the scattered `submit_*` methods.
    ///
    /// # Example
    ///
    /// ```ignore
    /// manager.task("task-1", "Analyze the code")
    ///     .with_provider(custom_provider)
    ///     .with_config(config)
    ///     .with_streaming(event_tx)
    ///     .spawn(result_tx)
    ///     .await?;
    /// ```
    pub fn task(
        &self,
        subagent_id: impl Into<String>,
        task: impl Into<String>,
    ) -> SubagentTaskBuilder<'_> {
        SubagentTaskBuilder::new(self, subagent_id.into(), task.into())
    }

    #[instrument(name = "subagent.submit", skip_all)]
    pub fn submit(&self, prompt: &str, channel: &str, chat_id: &str) -> anyhow::Result<()> {
        let channel_enum = match channel {
            "telegram" => gasket_types::ChannelType::Telegram,
            "discord" => gasket_types::ChannelType::Discord,
            "slack" => gasket_types::ChannelType::Slack,
            "email" => gasket_types::ChannelType::Email,
            "dingtalk" => gasket_types::ChannelType::Dingtalk,
            "feishu" => gasket_types::ChannelType::Feishu,
            _ => gasket_types::ChannelType::Cli,
        };
        let chat_id = chat_id.to_string();
        let outbound_tx = self.outbound_tx.clone();
        let provider = self.provider.clone();
        let workspace = self.workspace.clone();
        let tools = self.tools.clone();
        let prompt = prompt.to_string();

        tokio::spawn(async move {
            let config = AgentConfig {
                model: provider.default_model().to_string(),
                max_iterations: 10,
                ..Default::default()
            };

            let system_prompt = match resolve_system_prompt(&workspace, None).await {
                Ok(p) => p,
                Err(e) => {
                    warn!("Failed to load system prompt: {}", e);
                    return;
                }
            };

            let ctx = build_kernel_context(provider, &config, tools, None);
            let messages = build_kernel_messages(&system_prompt, &prompt);
            let timeout_duration = std::time::Duration::from_secs(SUBAGENT_TIMEOUT_SECS);
            let result = tokio::time::timeout(
                timeout_duration,
                kernel::execute(&ctx, messages),
            )
            .await;
            let content = match result {
                Ok(Ok(response)) => {
                    format!("Background task completed:\n{}", response.content)
                }
                Ok(Err(e)) => format!("Background task failed: {}", e),
                Err(_) => format!(
                    "Background task failed: Execution timed out after {:?}",
                    timeout_duration
                ),
            };
            let _ = outbound_tx
                .send(gasket_types::OutboundMessage {
                    channel: channel_enum,
                    chat_id,
                    content,
                    metadata: None,
                    trace_id: None,
                    ws_message: None,
                })
                .await;
        });

        Ok(())
    }

    /// Submit a prompt and **synchronously wait** for the agent response.
    ///
    /// Unlike `submit()` (fire-and-forget), this method blocks the caller
    /// until the subagent finishes. It is designed for governance-layer
    /// agents (e.g. review gates) where the pipeline must wait for a
    /// decision before proceeding.
    ///
    /// An optional `system_prompt` can be provided to inject a role-specific
    /// SOUL.md — if `None`, the default minimal bootstrap prompt is used.
    #[instrument(name = "subagent.submit_and_wait", skip_all)]
    pub async fn submit_and_wait(
        &self,
        prompt_text: &str,
        system_prompt: Option<&str>,
        _channel: &str,
        _chat_id: &str,
    ) -> anyhow::Result<AgentResponse> {
        info!("Subagent (sync) started: {}", &prompt_text);

        let agent_config = AgentConfig {
            model: self.provider.default_model().to_string(),
            max_iterations: 10,
            ..Default::default()
        };
        let model_name = agent_config.model.clone();

        let sys_prompt = resolve_system_prompt(&self.workspace, system_prompt).await?;
        let ctx = build_kernel_context(self.provider.clone(), &agent_config, self.tools.clone(), None);
        let messages = build_kernel_messages(&sys_prompt, prompt_text);

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(SUBAGENT_TIMEOUT_SECS),
            kernel::execute(&ctx, messages),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Subagent timed out after {SUBAGENT_TIMEOUT_SECS}s"))?
        .map_err(|e| anyhow::anyhow!("{}", e))?;

        Ok(to_agent_response(result, &model_name))
    }

    /// Submit a prompt with a **specific model** and wait for the response.
    ///
    /// This method allows switching to a different provider/model for the
    /// subagent execution. Used by the `switch_model` tool.
    #[instrument(name = "subagent.submit_and_wait_with_model", skip_all)]
    pub async fn submit_and_wait_with_model(
        &self,
        prompt_text: &str,
        system_prompt: Option<&str>,
        provider: Arc<dyn LlmProvider>,
        agent_config: AgentConfig,
        _channel: &str,
        _chat_id: &str,
    ) -> anyhow::Result<AgentResponse> {
        info!(
            "Subagent (model switch) started with model '{}': {}",
            agent_config.model, &prompt_text
        );
        let model_name = agent_config.model.clone();

        let sys_prompt = resolve_system_prompt(&self.workspace, system_prompt).await?;
        let ctx = build_kernel_context(provider, &agent_config, self.tools.clone(), None);
        let messages = build_kernel_messages(&sys_prompt, prompt_text);

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(SUBAGENT_TIMEOUT_SECS),
            kernel::execute(&ctx, messages),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Model switch task timed out after {SUBAGENT_TIMEOUT_SECS}s"))?
        .map_err(|e| anyhow::anyhow!("{}", e))?;

        Ok(to_agent_response(result, &model_name))
    }

    /// Submit a prompt with a **specific model** and stream events to a callback.
    ///
    /// This method allows switching to a different provider/model for the
    /// subagent execution with streaming support. Used by the `switch_model` tool
    /// to send real-time updates to WebSocket clients.
    #[instrument(name = "subagent.submit_and_wait_with_model_streaming", skip_all)]
    pub async fn submit_and_wait_with_model_streaming<F>(
        &self,
        prompt_text: &str,
        system_prompt: Option<&str>,
        provider: Arc<dyn LlmProvider>,
        agent_config: AgentConfig,
        mut stream_callback: F,
    ) -> anyhow::Result<AgentResponse>
    where
        F: FnMut(kernel::StreamEvent) + Send + 'static,
    {
        info!(
            "Subagent (model switch streaming) started with model '{}': {}",
            agent_config.model, prompt_text
        );
        let model_name = agent_config.model.clone();

        let sys_prompt = resolve_system_prompt(&self.workspace, system_prompt).await?;
        let ctx = build_kernel_context(provider, &agent_config, self.tools.clone(), None);
        let messages = build_kernel_messages(&sys_prompt, prompt_text);

        let (kernel_event_tx, mut event_rx) = mpsc::channel(64);
        let ctx_clone = ctx.clone();
        let model_name_owned = model_name.clone();

        let result_handle = tokio::spawn(async move {
            let result = kernel::execute_streaming(&ctx_clone, messages, kernel_event_tx)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            Ok::<AgentResponse, anyhow::Error>(to_agent_response(result, &model_name_owned))
        });

        let forward_handle = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                stream_callback(event);
            }
        });

        let timeout_result = tokio::time::timeout(
            std::time::Duration::from_secs(SUBAGENT_TIMEOUT_SECS),
            async {
                let (result, _) = tokio::join!(result_handle, forward_handle);
                result.map_err(|e| anyhow::anyhow!("{}", e))
            },
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!("Model switch task timed out after {SUBAGENT_TIMEOUT_SECS}s")
        })?;

        // timeout_result is Result<Result<AgentResponse, anyhow::Error>, anyhow::Error>
        // (JoinError was already converted to anyhow::Error inside the async block)
        let inner: Result<AgentResponse, anyhow::Error> = timeout_result
            .map_err(|e: anyhow::Error| anyhow::anyhow!("Join error: {}", e))?;

        inner
    }

    /// Send a WebSocket message to the outbound channel.
    ///
    /// This is a helper method for tools to send real-time updates.
    pub async fn send_ws_message(&self, msg: OutboundMessage) {
        if let Err(e) = self.outbound_tx.send(msg).await {
            warn!("Failed to send WebSocket message: {}", e);
        }
    }

    /// Try to send a WebSocket message without waiting.
    ///
    /// Uses try_send to avoid blocking if the channel is full.
    pub fn try_send_ws_message(&self, msg: OutboundMessage) -> bool {
        self.outbound_tx.try_send(msg).is_ok()
    }
}

// Step 5: Implement SubagentSpawner trait for SubagentManager
// This allows SubagentManager to be used as a spawner in ToolContext

use async_trait::async_trait;
use gasket_types::{SubagentResponse, SubagentResult as TypesSubagentResult, SubagentSpawner};

#[async_trait]
impl SubagentSpawner for SubagentManager {
    async fn spawn(
        &self,
        task: String,
        model_id: Option<String>,
    ) -> Result<TypesSubagentResult, Box<dyn std::error::Error + Send>> {
        use super::tracker::SubagentTracker;

        // Create tracker for single task
        let mut tracker = SubagentTracker::new();
        let result_tx = tracker.result_sender();
        let event_tx = tracker.event_sender();
        let subagent_id = SubagentTracker::generate_id();

        info!(
            "[SubagentSpawner] Starting subagent {} for task: {} (model_id: {:?})",
            subagent_id, task, model_id
        );

        // Prepare spawn configuration using Builder pattern
        let mut builder = self.task(subagent_id.clone(), task.clone());

        // Pass token_tracker to subagent for unified budget enforcement
        if let Some(ref token_tracker) = self.token_tracker {
            builder = builder.with_token_tracker(token_tracker.clone());
        }

        // Resolve model_id to provider and config if provided
        if let Some(ref mid) = model_id {
            if let Some(ref resolver) = self.model_resolver {
                if let Some((provider, config)) = resolver.resolve_model(mid) {
                    info!(
                        "[SubagentSpawner] Resolved model_id '{}' to provider with model '{}'",
                        mid, config.model
                    );
                    builder = builder.with_provider(provider).with_config(config);
                } else {
                    warn!(
                        "[SubagentSpawner] Could not resolve model_id '{}', using default provider",
                        mid
                    );
                }
            } else {
                warn!(
                    "[SubagentSpawner] model_id '{}' provided but no model_resolver available, using default provider",
                    mid
                );
            }
        }

        // Spawn the subagent
        let spawn_result = builder
            .with_streaming(event_tx.clone())
            .spawn(result_tx.clone())
            .await;

        // Check spawn result
        if let Err(e) = spawn_result {
            return Err(anyhow::anyhow!("Failed to spawn subagent: {}", e).into());
        }

        // Drop original senders - channel will close when all tasks complete
        drop(result_tx);
        drop(event_tx);

        // Wait for result
        let results = match tracker.wait_for_all(1).await {
            Ok(r) => r,
            Err(e) => {
                return Err(anyhow::anyhow!("Failed to wait for subagent results: {}", e).into())
            }
        };

        if results.is_empty() {
            return Err(anyhow::anyhow!("Subagent completed but no result was received").into());
        }

        let result = results.into_iter().next().unwrap();

        // Convert from crate::agent::subagent_tracker::SubagentResult to gasket_types::SubagentResult
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
