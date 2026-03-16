//! Spawn tool for subagent execution with synchronous blocking and streaming output
//!
//! This tool spawns a subagent and blocks until completion, streaming events
//! to the WebSocket/channel in real-time. This ensures the main agent waits
//! for results instead of using fire-and-forget semantics.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::time::timeout;
use tracing::{info, instrument, trace, warn};

use super::base::{Tool, ToolError};
use crate::agent::stream_buffer::BufferedEvents;
use crate::agent::subagent::SubagentManager;
use crate::agent::subagent_tracker::{SubagentEvent, SubagentTracker};
use crate::bus::events::{OutboundMessage, WebSocketMessage};
use crate::config::ModelRegistry;
use crate::providers::ProviderRegistry;

pub struct SpawnTool {
    manager: Option<Arc<SubagentManager>>,
    model_registry: Option<Arc<ModelRegistry>>,
    provider_registry: Option<Arc<ProviderRegistry>>,
}

impl SpawnTool {
    pub fn new() -> Self {
        Self {
            manager: None,
            model_registry: None,
            provider_registry: None,
        }
    }

    pub fn with_manager(manager: Arc<SubagentManager>) -> Self {
        Self {
            manager: Some(manager),
            model_registry: None,
            provider_registry: None,
        }
    }

    pub fn with_registries(
        manager: Arc<SubagentManager>,
        model_registry: Arc<ModelRegistry>,
        provider_registry: Arc<ProviderRegistry>,
    ) -> Self {
        Self {
            manager: Some(manager),
            model_registry: Some(model_registry),
            provider_registry: Some(provider_registry),
        }
    }

    /// Select model based on model_id with fallback and optional smart selection.
    ///
    /// Selection logic:
    /// 1. If model_id is provided, try exact match first
    /// 2. If not found, fallback to default model
    /// 3. If no model_id and smart-model-selection feature is enabled,
    ///    analyze task content and select by capability
    /// 4. Otherwise use default model
    /// 5. Return None if no model profile matches (use manager default)
    fn select_model<'a>(
        &'a self,
        model_id: &'a Option<String>,
        _task: &str,
    ) -> Option<(&'a str, &'a crate::config::ModelProfile)> {
        let model_registry = self.model_registry.as_ref()?;

        match model_id {
            Some(id) => {
                // Try exact match with fallback to default
                model_registry.get_profile_with_fallback(Some(id))
            }
            None => {
                // No model_id specified
                #[cfg(feature = "smart-model-selection")]
                {
                    // Smart selection based on task content
                    model_registry.select_by_capability(_task)
                }
                #[cfg(not(feature = "smart-model-selection"))]
                {
                    // Use default model
                    model_registry.get_profile_with_fallback(None)
                }
            }
        }
    }
}

impl Default for SpawnTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct SpawnArgs {
    task: String,
    model_id: Option<String>,
}

#[async_trait]
impl Tool for SpawnTool {
    fn name(&self) -> &str {
        "spawn"
    }

    fn description(&self) -> &str {
        "Spawn a subagent to execute a task synchronously with real-time streaming output. \
         The main agent blocks until the subagent completes and returns the result. \
         Use this for tasks that need immediate results with progress feedback."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Task description / prompt to execute"
                },
                "model_id": {
                    "type": "string",
                    "description": "Optional model profile ID to use for this subagent. If not specified, uses the default model."
                }
            },
            "required": ["task"]
        })
    }

    #[instrument(name = "tool.spawn", skip_all)]
    async fn execute(&self, args: Value) -> Result<String, ToolError> {
        let args: SpawnArgs =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let manager = match &self.manager {
            Some(m) => m,
            None => {
                return Err(ToolError::ExecutionError(
                    "Subagent spawning is not available in this mode.".to_string(),
                ))
            }
        };

        if args.task.trim().is_empty() {
            return Err(ToolError::InvalidArguments(
                "Task description cannot be empty".to_string(),
            ));
        }

        // Create tracker for single task
        let mut tracker = SubagentTracker::new();
        let result_tx = tracker.result_sender();
        let event_tx = tracker.event_sender();
        let subagent_id = SubagentTracker::generate_id();
        let task = args.task.clone();

        info!(
            "[Spawn] Starting subagent {} for task: {}",
            subagent_id, task
        );

        // Prepare spawn configuration using Builder pattern
        let mut builder = manager.task(subagent_id.clone(), task.clone());

        // Model selection logic with fallback and optional smart selection
        let selected_model = self.select_model(&args.model_id, &args.task);

        // Apply selected model configuration if available
        if let Some((profile_id, profile)) = selected_model {
            let provider_registry = self.provider_registry.as_ref().ok_or_else(|| {
                ToolError::ExecutionError("Provider registry not available".to_string())
            })?;

            let provider = provider_registry
                .get_or_create(&profile.provider)
                .map_err(|e| {
                    ToolError::ExecutionError(format!("Failed to create provider: {}", e))
                })?;

            let agent_config = crate::agent::loop_::AgentConfig {
                model: profile.model.clone(),
                temperature: profile.temperature.unwrap_or(0.7),
                thinking_enabled: profile.thinking_enabled.unwrap_or(false),
                max_tokens: profile.max_tokens.unwrap_or(4096),
                ..Default::default()
            };

            builder = builder.with_provider(provider).with_config(agent_config);
            info!(
                "[Spawn] Using model profile '{}' (model: {}) for subagent {}",
                profile_id, profile.model, subagent_id
            );
        } else {
            info!(
                "[Spawn] Using default provider for subagent {} (no model profile matched)",
                subagent_id
            );
        }

        let spawn_result = builder
            .with_streaming(event_tx.clone())
            .spawn(result_tx.clone())
            .await;

        // Check spawn result
        if let Err(e) = spawn_result {
            return Err(ToolError::ExecutionError(format!(
                "Failed to spawn subagent: {}",
                e
            )));
        }

        // Drop original senders - channel will close when all tasks complete
        drop(result_tx);
        drop(event_tx);

        // Take event receiver for streaming
        let mut event_rx = tracker.take_event_receiver().map_err(|e| {
            ToolError::ExecutionError(format!("Failed to take event receiver: {}", e))
        })?;

        // Get outbound channel and session key for WebSocket streaming
        let outbound_tx = manager.outbound_sender();
        let session_key = manager.get_session_key();

        // Track whether we're at the start of a new line (for prefix insertion)
        let mut at_line_start = true;

        // Buffer events until subagent completes
        let mut buffer = BufferedEvents::new();

        // Spawn background task to collect events and forward to WebSocket/channel
        // Uses buffering: collect all events, send them only when subagent completes
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                // Log the event
                match &event {
                    SubagentEvent::Started { id, task } => {
                        info!("[Spawn] Subagent {} started: {}", id, task);
                        at_line_start = true;
                    }
                    SubagentEvent::Thinking { id, content } => {
                        trace!("[Spawn] Subagent {} thinking: {}", id, content);
                    }
                    SubagentEvent::Content { id, content } => {
                        trace!("[Spawn] Subagent {} content: {} bytes", id, content.len());
                    }
                    SubagentEvent::Iteration { id, iteration } => {
                        info!("[Spawn] Subagent {} iteration {} completed", id, iteration);
                        at_line_start = true;
                    }
                    SubagentEvent::ToolStart { id, tool_name, .. } => {
                        trace!("[Spawn] Subagent {} tool: {} started", id, tool_name);
                    }
                    SubagentEvent::ToolEnd { id, tool_name, .. } => {
                        trace!("[Spawn] Subagent {} tool: {} done", id, tool_name);
                        at_line_start = true;
                    }
                    SubagentEvent::Completed { id, result } => {
                        info!(
                            "[Spawn] Subagent {} completed, model={}",
                            id,
                            result.model.as_deref().unwrap_or("unknown")
                        );
                        buffer.completed = true;
                    }
                    SubagentEvent::Error { id, error } => {
                        warn!("[Spawn] Subagent {} error: {}", id, error);
                        buffer.completed = true;
                    }
                }

                // Buffer WebSocket message (don't send immediately)
                if session_key.is_some() {
                    let ws_msg = match &event {
                        SubagentEvent::Thinking { content, .. } => {
                            // Only add prefix at line start to avoid repeating for every char
                            let msg = if at_line_start || content.starts_with('\n') {
                                format!("[Subagent] {}", content.trim_start())
                            } else {
                                content.clone()
                            };
                            // Update line start state based on content
                            at_line_start = content.ends_with('\n');
                            Some(WebSocketMessage::thinking(msg))
                        }
                        SubagentEvent::Content { content, .. } => {
                            // Only add prefix at line start
                            let msg = if at_line_start || content.starts_with('\n') {
                                format!("[Subagent] {}", content.trim_start())
                            } else {
                                content.clone()
                            };
                            // Update line start state based on content
                            at_line_start = content.ends_with('\n');
                            Some(WebSocketMessage::content(msg))
                        }
                        SubagentEvent::Iteration { iteration, .. } => Some(WebSocketMessage::text(
                            format!("[Subagent] Iteration {} completed", iteration),
                        )),
                        SubagentEvent::ToolStart {
                            tool_name,
                            arguments,
                            ..
                        } => Some(WebSocketMessage::tool_start(
                            format!("[Subagent] {}", tool_name),
                            arguments.clone(),
                        )),
                        SubagentEvent::ToolEnd {
                            tool_name, output, ..
                        } => Some(WebSocketMessage::tool_end(
                            format!("[Subagent] {}", tool_name),
                            Some(output.clone()),
                        )),
                        SubagentEvent::Error { error, .. } => Some(WebSocketMessage::text(
                            format!("[Subagent Error] {}", error),
                        )),
                        _ => None, // Started, Completed - don't send to WS
                    };

                    if let Some(msg) = ws_msg {
                        buffer.messages.push(msg);
                    }
                }

                // When subagent completes, flush all buffered messages in ordered format
                if buffer.completed {
                    if let Some(ref key) = session_key {
                        // Use flush_ordered to ensure Thinking messages come before Content
                        for msg in buffer.flush_ordered() {
                            let outbound = OutboundMessage::with_ws_message(
                                key.channel.clone(),
                                &key.chat_id,
                                msg,
                            );
                            // Use timeout + send to apply backpressure without indefinite blocking
                            match timeout(Duration::from_millis(100), outbound_tx.send(outbound))
                                .await
                            {
                                Ok(Ok(_)) => { /* sent successfully */ }
                                Ok(Err(e)) => warn!("[Spawn] Outbound channel closed: {}", e),
                                Err(_) => warn!(
                                    "[Spawn] Send timeout after 100ms, outbound channel congested"
                                ),
                            }
                        }
                    }
                    buffer.completed = false;
                }
            }
        });

        // Wait for result (blocking)
        info!("[Spawn] Waiting for subagent result...");
        let results = tracker.wait_for_all(1).await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to wait for subagent results: {}", e))
        })?;

        if results.is_empty() {
            return Err(ToolError::ExecutionError(
                "Subagent completed but no result was received".to_string(),
            ));
        }

        let result = results.into_iter().next().unwrap();

        // Format output
        let mut output = String::new();

        // Include thinking content if available
        if let Some(ref reasoning) = result.response.reasoning_content {
            if !reasoning.is_empty() {
                output.push_str(&format!("**Thinking:**\n{}\n\n", reasoning));
            }
        }

        output.push_str(&format!(
            "**Model:** {}\n**Task:** {}\n\n**Response:**\n{}",
            result.model.as_deref().unwrap_or("unknown"),
            result.task,
            result.response.content
        ));

        Ok(output)
    }
}
