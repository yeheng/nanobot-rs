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

use crate::agent::executor_core::{AgentExecutor, ExecutionResult};
use crate::agent::loop_::AgentConfig;
use crate::agent::prompt;
use crate::agent::stream::StreamEvent;
use crate::agent::subagent_tracker::{SubagentEvent, SubagentResult};
use crate::hooks::HookRegistry;
use crate::tools::ToolRegistry;
use gasket_bus::events::{OutboundMessage, SessionKey};
use gasket_providers::{ChatMessage, LlmProvider};

use super::loop_::{AgentLoop, AgentResponse};

/// Default timeout for subagent execution (10 minutes)
const SUBAGENT_TIMEOUT_SECS: u64 = 600;

/// Trait for resolving model IDs to providers and configs.
///
/// Implemented by the CLI layer using `ProviderRegistry` + `ModelRegistry`.
/// This decouples the engine from configuration details.
pub trait ModelResolver: Send + Sync {
    /// Resolve a model ID to a provider and agent config.
    ///
    /// Returns `None` if the model ID is not recognized.
    fn resolve_model(&self, model_id: &str) -> Option<(Arc<dyn LlmProvider>, AgentConfig)>;
}

/// Run a subagent with minimal overhead - pure function
pub async fn run_subagent(
    task: &str,
    system_prompt: &str,
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    config: &AgentConfig,
) -> Result<ExecutionResult, anyhow::Error> {
    let messages = vec![ChatMessage::system(system_prompt), ChatMessage::user(task)];
    let executor = AgentExecutor::new(provider, tools, config);
    executor
        .execute(messages)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
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
    /// Optional hooks registry (uses empty registry if None)
    hooks: Option<Arc<HookRegistry>>,
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
            hooks: None,
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

    /// Set custom hooks for this subagent.
    ///
    /// When set, the subagent will use these hooks instead of an empty registry.
    /// This allows subagents to have their own hook pipeline (e.g., for logging,
    /// auditing, or custom preprocessing).
    pub fn with_hooks(mut self, hooks: Arc<HookRegistry>) -> Self {
        self.hooks = Some(hooks);
        self
    }

    /// Inherit hooks from the main agent.
    ///
    /// This is a convenience method that's equivalent to `with_hooks(agent_hooks)`.
    /// Use this when you want the subagent to share the same hook pipeline as
    /// the main agent (e.g., for shared logging or vault access).
    pub fn inherit_hooks(mut self, agent_hooks: Arc<HookRegistry>) -> Self {
        self.hooks = Some(agent_hooks);
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
        let hooks = self.hooks.clone();
        let event_tx = self.event_tx.clone();
        let system_prompt_override = self.system_prompt;

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

            // Initialize agent
            let agent =
                match AgentLoop::for_subagent(provider, workspace.clone(), agent_config, tools) {
                    Ok(a) => a,
                    Err(e) => {
                        warn!("[Subagent {}] Failed to initialize: {}", subagent_id, e);
                        Self::send_initialization_error(
                            &subagent_id,
                            &task,
                            &model_name,
                            &e.to_string(),
                            &event_tx,
                            &result_tx,
                        )
                        .await;
                        return;
                    }
                };

            // Apply hooks if provided
            let mut agent = if let Some(ref hooks) = hooks {
                agent.with_hooks(hooks.clone())
            } else {
                agent
            };

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
            agent.set_system_prompt(system_prompt);

            // Execute with timeout
            let session_key = SessionKey::new(gasket_types::ChannelType::Cli, &subagent_id);
            let response = Self::execute_with_timeout(
                agent,
                &task,
                &session_key,
                &event_tx,
                &subagent_id,
                &cancellation_token,
            )
            .await;

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

    /// Execute agent with timeout and cancellation support.
    async fn execute_with_timeout(
        agent: AgentLoop,
        task: &str,
        session_key: &SessionKey,
        event_tx: &Option<mpsc::Sender<SubagentEvent>>,
        subagent_id: &str,
        cancellation_token: &Option<tokio_util::sync::CancellationToken>,
    ) -> Result<AgentResponse, anyhow::Error> {
        let timeout_duration = std::time::Duration::from_secs(SUBAGENT_TIMEOUT_SECS);

        if let Some(tx) = event_tx {
            // Streaming path
            Self::execute_streaming(
                agent,
                task,
                session_key,
                tx,
                subagent_id,
                cancellation_token,
                timeout_duration,
            )
            .await
        } else {
            // Non-streaming path
            match tokio::time::timeout(timeout_duration, agent.process_direct(task, session_key))
                .await
            {
                Ok(Ok(resp)) => Ok(resp),
                Ok(Err(e)) => Err(anyhow::anyhow!("Execution failed: {}", e)),
                Err(_) => Err(anyhow::anyhow!("Timed out after {:?}", timeout_duration)),
            }
        }
    }

    /// Execute agent with streaming support.
    async fn execute_streaming(
        agent: AgentLoop,
        task: &str,
        session_key: &SessionKey,
        event_tx: &mpsc::Sender<SubagentEvent>,
        subagent_id: &str,
        cancellation_token: &Option<tokio_util::sync::CancellationToken>,
        timeout_duration: std::time::Duration,
    ) -> Result<AgentResponse, anyhow::Error> {
        let tx_clone = event_tx.clone();
        let id_clone = subagent_id.to_string();
        let iteration_counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let cancellation_token_clone = cancellation_token.clone();

        let (mut event_rx, result_handle) = agent
            .process_direct_streaming_with_channel(task, session_key)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create streaming channel: {}", e))?;

        let forward_handle = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                match event {
                    StreamEvent::Content(content) => {
                        let _ = tx_clone.try_send(SubagentEvent::Content {
                            id: id_clone.clone(),
                            content,
                        });
                    }
                    StreamEvent::Reasoning(content) => {
                        let _ = tx_clone.try_send(SubagentEvent::Thinking {
                            id: id_clone.clone(),
                            content,
                        });
                    }
                    StreamEvent::ToolStart { name, arguments } => {
                        let _ = tx_clone.try_send(SubagentEvent::ToolStart {
                            id: id_clone.clone(),
                            tool_name: name,
                            arguments,
                        });
                    }
                    StreamEvent::ToolEnd { name, output } => {
                        let _ = tx_clone.try_send(SubagentEvent::ToolEnd {
                            id: id_clone.clone(),
                            tool_name: name,
                            output,
                        });
                    }
                    StreamEvent::Done => {
                        let iter = iteration_counter
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                            + 1;
                        let _ = tx_clone.try_send(SubagentEvent::Iteration {
                            id: id_clone.clone(),
                            iteration: iter,
                        });
                    }
                    StreamEvent::TokenStats { .. } => {}
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
                    result
                        .map_err(|_| anyhow::anyhow!("timed out"))
                        .and_then(|r| r.map_err(|e| anyhow::anyhow!("Task join error: {}", e)))
                        .and_then(|r| r.map_err(|e| anyhow::anyhow!("Execution failed: {}", e)))
                }
            }
        } else {
            tokio::time::timeout(timeout_duration, result_handle)
                .await
                .map_err(|_| anyhow::anyhow!("timed out"))
                .and_then(|r| r.map_err(|e| anyhow::anyhow!("Task join error: {}", e)))
                .and_then(|r| r.map_err(|e| anyhow::anyhow!("Execution failed: {}", e)))
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
            timeout_secs: super::loop_::DEFAULT_SUBAGENT_TIMEOUT_SECS,
            model_resolver,
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
        let provider = self.provider.clone();
        let workspace = self.workspace.clone();
        let tools = self.tools.clone();
        let outbound_tx = self.outbound_tx.clone();
        let prompt = prompt.to_string();

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
        let session_key = SessionKey::new(channel_enum.clone(), &chat_id);

        tokio::spawn(async move {
            info!("Subagent task started: {}", prompt);
            let agent_config = AgentConfig {
                model: provider.default_model().to_string(),
                max_iterations: 10,
                ..Default::default()
            };

            let mut agent =
                match AgentLoop::for_subagent(provider, workspace.clone(), agent_config, tools) {
                    Ok(a) => a,
                    Err(e) => {
                        warn!("Failed to initialise subagent: {}", e);
                        return;
                    }
                };

            // Load minimal system prompt directly (no hook dispatch)
            let system_prompt =
                match prompt::load_system_prompt(&workspace, prompt::BOOTSTRAP_FILES_MINIMAL).await
                {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("Failed to load minimal system prompt: {}", e);
                        return;
                    }
                };
            agent.set_system_prompt(system_prompt);

            let timeout_duration = std::time::Duration::from_secs(SUBAGENT_TIMEOUT_SECS);
            let result = tokio::time::timeout(
                timeout_duration,
                agent.process_direct(&prompt, &session_key),
            )
            .await;

            let content = match result {
                Ok(Ok(response)) => format!("Background task completed:\n{}", response.content),
                Ok(Err(e)) => format!("Background task failed: {}", e),
                Err(_) => format!(
                    "Background task failed: Execution timed out after {:?}",
                    timeout_duration
                ),
            };

            let msg = gasket_types::OutboundMessage {
                channel: channel_enum,
                chat_id,
                content,
                metadata: None,
                trace_id: None,
                ws_message: None,
            };

            // Route through the Outbound Actor — no direct HTTP call
            if let Err(e) = outbound_tx.send(msg).await {
                warn!("Failed to send subagent result to outbound channel: {}", e);
            }
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
        channel: &str,
        chat_id: &str,
    ) -> anyhow::Result<AgentResponse> {
        info!("Subagent (sync) started: {}", &prompt_text);

        let agent_config = AgentConfig {
            model: self.provider.default_model().to_string(),
            max_iterations: 10,
            ..Default::default()
        };

        let mut agent = AgentLoop::for_subagent(
            self.provider.clone(),
            self.workspace.clone(),
            agent_config,
            self.tools.clone(),
        )?;

        let sys = match system_prompt {
            Some(s) => s.to_string(),
            None => {
                prompt::load_system_prompt(&self.workspace, prompt::BOOTSTRAP_FILES_MINIMAL).await?
            }
        };
        agent.set_system_prompt(sys);

        let channel_enum = gasket_types::ChannelType::new(channel);
        let session_key = SessionKey::new(channel_enum, chat_id);

        tokio::time::timeout(
            std::time::Duration::from_secs(SUBAGENT_TIMEOUT_SECS),
            agent.process_direct(prompt_text, &session_key),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Subagent timed out after {SUBAGENT_TIMEOUT_SECS}s"))?
        .map_err(|e| anyhow::anyhow!("{}", e))
    }

    /// Submit a prompt with a **specific model** and wait for the response.
    ///
    /// This method allows switching to a different provider/model for the
    /// subagent execution. Used by the `switch_model` tool.
    ///
    /// # Arguments
    /// * `prompt_text` - The task description for the subagent
    /// * `system_prompt` - Optional custom system prompt (uses minimal bootstrap if None)
    /// * `provider` - The LLM provider to use for this execution
    /// * `agent_config` - Agent configuration including model, temperature, etc.
    #[instrument(name = "subagent.submit_and_wait_with_model", skip_all)]
    pub async fn submit_and_wait_with_model(
        &self,
        prompt_text: &str,
        system_prompt: Option<&str>,
        provider: Arc<dyn LlmProvider>,
        agent_config: AgentConfig,
        channel: &str,
        chat_id: &str,
    ) -> anyhow::Result<AgentResponse> {
        info!(
            "Subagent (model switch) started with model '{}': {}",
            agent_config.model, &prompt_text
        );

        let mut agent = AgentLoop::for_subagent(
            provider,
            self.workspace.clone(),
            agent_config,
            self.tools.clone(),
        )?;

        let sys = match system_prompt {
            Some(s) => s.to_string(),
            None => {
                prompt::load_system_prompt(&self.workspace, prompt::BOOTSTRAP_FILES_MINIMAL).await?
            }
        };
        agent.set_system_prompt(sys);

        let channel_enum = gasket_types::ChannelType::new(channel);
        let session_key = SessionKey::new(channel_enum, chat_id);

        tokio::time::timeout(
            std::time::Duration::from_secs(SUBAGENT_TIMEOUT_SECS),
            agent.process_direct(prompt_text, &session_key),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Model switch task timed out after {SUBAGENT_TIMEOUT_SECS}s"))?
        .map_err(|e| anyhow::anyhow!("{}", e))
    }

    /// Submit a prompt with a **specific model** and stream events to a callback.
    ///
    /// This method allows switching to a different provider/model for the
    /// subagent execution with streaming support. Used by the `switch_model` tool
    /// to send real-time updates to WebSocket clients.
    ///
    /// # Arguments
    /// * `prompt_text` - The task description for the subagent
    /// * `system_prompt` - Optional custom system prompt (uses minimal bootstrap if None)
    /// * `provider` - The LLM provider to use for this execution
    /// * `agent_config` - Agent configuration including model, temperature, etc.
    /// * `stream_callback` - Callback function for streaming events
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
        F: FnMut(StreamEvent) + Send + 'static,
    {
        info!(
            "Subagent (model switch streaming) started with model '{}': {}",
            agent_config.model, prompt_text
        );

        let mut agent = AgentLoop::for_subagent(
            provider,
            self.workspace.clone(),
            agent_config,
            self.tools.clone(),
        )?;

        let sys = match system_prompt {
            Some(s) => s.to_string(),
            None => {
                prompt::load_system_prompt(&self.workspace, prompt::BOOTSTRAP_FILES_MINIMAL).await?
            }
        };
        agent.set_system_prompt(sys);

        let session_key = SessionKey::new(gasket_types::ChannelType::Cli, "model_switch_streaming");

        // Use channel-based streaming API
        let (mut event_rx, result_handle) = agent
            .process_direct_streaming_with_channel(prompt_text, &session_key)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        // Forward events to callback
        let forward_handle = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                stream_callback(event);
            }
        });

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(SUBAGENT_TIMEOUT_SECS),
            async {
                let (result, _) = tokio::join!(result_handle, forward_handle);
                result.map_err(|e| anyhow::anyhow!("{}", e))
            },
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!("Model switch task timed out after {SUBAGENT_TIMEOUT_SECS}s")
        })??;

        Ok(result?)
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
        use crate::agent::subagent_tracker::SubagentTracker;

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
