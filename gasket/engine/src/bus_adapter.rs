//! Adapter for integrating with gasket-bus

use crate::bus::MessageHandler;
use crate::session::AgentSession;
use async_trait::async_trait;
use gasket_types::SessionKey;
use std::sync::Arc;

/// Engine handler for bus integration.
pub struct EngineHandler {
    session: Arc<AgentSession>,
}

impl EngineHandler {
    /// Create a new engine handler.
    pub fn new(session: Arc<AgentSession>) -> Self {
        Self { session }
    }

    /// Get the underlying session.
    pub fn session(&self) -> &AgentSession {
        &self.session
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
            .session
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

        // Get the streaming result from AgentSession
        let (mut agent_event_rx, result_handle) = self
            .session
            .process_direct_streaming_with_channel(message, session_key)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        // Spawn a task to convert kernel StreamEvents to gasket_bus StreamEvents
        tokio::spawn(async move {
            use crate::bus::StreamEvent as BusStreamEvent;
            use gasket_types::StreamEvent;

            while let Some(event) = agent_event_rx.recv().await {
                // Skip subagent events - only forward main agent events
                if event.is_subagent_event() {
                    continue;
                }

                let bus_event = match event {
                    StreamEvent::Content {
                        agent_id: _,
                        content,
                    } => BusStreamEvent::Content(content),
                    StreamEvent::Thinking {
                        agent_id: _,
                        content,
                    } => BusStreamEvent::Reasoning(content),
                    StreamEvent::ToolStart {
                        agent_id: _,
                        name,
                        arguments,
                    } => BusStreamEvent::ToolStart {
                        name,
                        arguments: arguments.unwrap_or_default(),
                    },
                    StreamEvent::ToolEnd {
                        agent_id: _,
                        name,
                        output,
                    } => BusStreamEvent::ToolEnd {
                        name,
                        output: output.unwrap_or_default(),
                    },
                    StreamEvent::Done { agent_id: _ } => BusStreamEvent::Done,
                    StreamEvent::TokenStats {
                        input_tokens,
                        output_tokens,
                        total_tokens,
                        ..
                    } => BusStreamEvent::TokenStats {
                        prompt: input_tokens,
                        completion: output_tokens,
                        total: total_tokens,
                    },
                    // Subagent lifecycle events are filtered above
                    _ => continue,
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
