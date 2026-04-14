//! Adapter for integrating with gasket-broker.
//!
//! Implements the broker's `MessageHandler` trait for `EngineHandler`,
//! bridging AgentSession to the broker-based pipeline.

use std::sync::Arc;

use async_trait::async_trait;
use gasket_types::SessionKey;

use crate::session::AgentSession;

/// Engine handler for broker integration.
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
impl gasket_broker::session::MessageHandler for EngineHandler {
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
            tokio::sync::mpsc::Receiver<gasket_broker::session::StreamEvent>,
            tokio::sync::oneshot::Receiver<
                Result<gasket_types::events::OutboundMessage, Box<dyn std::error::Error + Send + Sync>>,
            >,
        ),
        Box<dyn std::error::Error + Send + Sync>,
    > {
        use gasket_broker::session::StreamEvent as BrokerEvent;

        let session_key_owned = session_key.clone();

        // Get the streaming result from AgentSession
        let (mut agent_event_rx, result_handle) = self
            .session
            .process_direct_streaming_with_channel(message, session_key)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        // Spawn a task to convert kernel StreamEvents to broker StreamEvents
        let (broker_tx, broker_rx) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            use gasket_types::StreamEvent;

            while let Some(event) = agent_event_rx.recv().await {
                // Skip subagent events - only forward main agent events
                if event.is_subagent_event() {
                    continue;
                }

                let broker_event = match event {
                    StreamEvent::Content {
                        agent_id: _,
                        content,
                    } => BrokerEvent::Content(content),
                    StreamEvent::Thinking {
                        agent_id: _,
                        content,
                    } => BrokerEvent::Reasoning(content),
                    StreamEvent::ToolStart {
                        agent_id: _,
                        name,
                        arguments,
                    } => BrokerEvent::ToolStart {
                        name,
                        arguments: arguments.unwrap_or_default(),
                    },
                    StreamEvent::ToolEnd {
                        agent_id: _,
                        name,
                        output,
                    } => BrokerEvent::ToolEnd {
                        name,
                        output: output.unwrap_or_default(),
                    },
                    StreamEvent::Done { agent_id: _ } => BrokerEvent::Done,
                    StreamEvent::TokenStats {
                        input_tokens,
                        output_tokens,
                        total_tokens,
                        ..
                    } => BrokerEvent::TokenStats {
                        prompt: input_tokens,
                        completion: output_tokens,
                        total: total_tokens,
                    },
                    // Subagent lifecycle events are filtered above
                    _ => continue,
                };

                if broker_tx.send(broker_event).await.is_err() {
                    break;
                }
            }
        });

        // Spawn a task to wrap the final result
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            match result_handle.await {
                Ok(Ok(response)) => {
                    let outbound_msg = gasket_types::events::OutboundMessage {
                        channel: gasket_types::events::ChannelType::Cli,
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

        Ok((broker_rx, result_rx))
    }
}
