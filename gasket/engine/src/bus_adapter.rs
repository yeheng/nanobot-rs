//! Adapter for integrating with gasket-bus

use crate::agent::AgentLoop;
use crate::bus::MessageHandler;
use async_trait::async_trait;
use gasket_types::SessionKey;
use std::sync::Arc;

/// Engine handler for bus integration.
pub struct EngineHandler {
    agent_loop: Arc<AgentLoop>,
}

impl EngineHandler {
    /// Create a new engine handler.
    pub fn new(agent_loop: Arc<AgentLoop>) -> Self {
        Self { agent_loop }
    }

    /// Get the underlying agent loop.
    pub fn agent_loop(&self) -> &AgentLoop {
        &self.agent_loop
    }
}

#[async_trait]
impl MessageHandler for EngineHandler {
    async fn handle_message(
        &self,
        session_key: &SessionKey,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let response = self
            .agent_loop
            .process_direct(message, session_key)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        Ok(response.content)
    }

    async fn handle_streaming_message(
        &self,
        message: &str,
        session_key: &SessionKey,
    ) -> Result<
        (
            tokio::sync::mpsc::Receiver<crate::bus::StreamEvent>,
            tokio::sync::oneshot::Receiver<
                Result<gasket_types::OutboundMessage, Box<dyn std::error::Error + Send + Sync>>,
            >,
        ),
        Box<dyn std::error::Error + Send + Sync>,
    > {
        use tokio::sync::mpsc;

        let (event_tx, event_rx) = mpsc::channel(32);
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();

        // Clone session_key for the spawned task
        let session_key_owned = session_key.clone();

        // Get the streaming result from AgentLoop
        let (mut agent_event_rx, result_handle) = self
            .agent_loop
            .process_direct_streaming_with_channel(message, session_key)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        // Spawn a task to convert AgentLoop StreamEvents to gasket_bus StreamEvents
        tokio::spawn(async move {
            use crate::agent::streaming::stream::StreamEvent as AgentStreamEvent;
            use crate::bus::StreamEvent as BusStreamEvent;

            while let Some(event) = agent_event_rx.recv().await {
                let bus_event = match event {
                    AgentStreamEvent::Content(content) => BusStreamEvent::Content(content),
                    AgentStreamEvent::Reasoning(content) => BusStreamEvent::Reasoning(content),
                    AgentStreamEvent::ToolStart { name, arguments } => BusStreamEvent::ToolStart {
                        name,
                        arguments: arguments.unwrap_or_default(),
                    },
                    AgentStreamEvent::ToolEnd { name, output } => {
                        BusStreamEvent::ToolEnd { name, output }
                    }
                    AgentStreamEvent::Done => BusStreamEvent::Done,
                    AgentStreamEvent::TokenStats {
                        input_tokens,
                        output_tokens,
                        total_tokens,
                        ..
                    } => BusStreamEvent::TokenStats {
                        prompt: input_tokens,
                        completion: output_tokens,
                        total: total_tokens,
                    },
                };

                if event_tx.send(bus_event).await.is_err() {
                    break;
                }
            }
        });

        // Spawn a task to wrap the final result
        tokio::spawn(async move {
            match result_handle.await {
                Ok(Ok(response)) => {
                    // Create an OutboundMessage from the response
                    let outbound_msg = gasket_types::OutboundMessage {
                        channel: gasket_types::ChannelType::Cli,
                        chat_id: session_key_owned.to_string(),
                        content: response.content,
                        metadata: None,
                        trace_id: None,
                        ws_message: None,
                    };
                    let _ = result_tx.send(Ok(outbound_msg));
                }
                Ok(Err(e)) => {
                    let _ = result_tx
                        .send(Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>));
                }
                Err(e) => {
                    let _ = result_tx
                        .send(Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>));
                }
            }
        });

        Ok((event_rx, result_rx))
    }
}
