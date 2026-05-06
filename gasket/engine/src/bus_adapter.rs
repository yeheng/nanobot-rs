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
        use crate::session::HandleOutcome;
        let outcome = self
            .session
            .handle_inbound(message, session_key, None)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        Ok(match outcome {
            HandleOutcome::Consumed => String::new(),
            HandleOutcome::Replied(resp) => resp.content,
        })
    }

    async fn handle_streaming_message(
        &self,
        message: &str,
        session_key: &SessionKey,
        tool_filter: Option<Vec<String>>,
    ) -> Result<
        (
            tokio::sync::mpsc::Receiver<gasket_types::events::ChatEvent>,
            tokio::sync::oneshot::Receiver<
                Result<
                    gasket_types::events::OutboundMessage,
                    Box<dyn std::error::Error + Send + Sync>,
                >,
            >,
        ),
        Box<dyn std::error::Error + Send + Sync>,
    > {
        let session_key_owned = session_key.clone();

        use crate::session::HandleOutcomeStreaming;
        let outcome = self
            .session
            .handle_inbound_streaming_with_channel(message, session_key, tool_filter)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        let (chat_rx, result_handle) = match outcome {
            HandleOutcomeStreaming::Consumed => {
                let (_chat_tx, chat_rx) = tokio::sync::mpsc::channel(1);
                let (result_tx, result_rx) = tokio::sync::oneshot::channel();
                let outbound_msg = gasket_types::events::OutboundMessage::new(
                    gasket_types::events::ChannelType::Cli,
                    session_key.to_string(),
                    String::new(),
                );
                let _ = result_tx.send(Ok(outbound_msg));
                return Ok((chat_rx, result_rx));
            }
            HandleOutcomeStreaming::Replied { events, result } => (events, result),
        };

        // Spawn a task to wrap the final result
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            match result_handle.await {
                Ok(Ok(response)) => {
                    let outbound_msg = gasket_types::events::OutboundMessage::new(
                        gasket_types::events::ChannelType::Cli,
                        session_key_owned.to_string(),
                        response.content,
                    );
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

        Ok((chat_rx, result_rx))
    }

    async fn handle_command(
        &self,
        session_key: &SessionKey,
        command: &str,
    ) -> Result<Vec<gasket_types::events::ChatEvent>, Box<dyn std::error::Error + Send + Sync>>
    {
        let mut events = Vec::new();
        if command == "force_compact" {
            let triggered = self.session.force_compact(session_key, &[]);
            if triggered {
                events.push(gasket_types::events::ChatEvent::text(
                    "Context compaction triggered.",
                ));
            } else {
                events.push(gasket_types::events::ChatEvent::text(
                    "Context compaction already in progress or not available.",
                ));
            }
        }
        // Always include latest stats
        if let Some(stats) = self.session.get_context_stats(session_key).await {
            events.push(gasket_types::events::ChatEvent::ContextStats {
                token_budget: stats.token_budget,
                compaction_threshold: stats.compaction_threshold as f64,
                threshold_tokens: stats.threshold_tokens,
                current_tokens: stats.current_tokens,
                usage_percent: stats.usage_percent,
                is_compressing: stats.is_compressing,
            });
        }
        if let Some(info) = self.session.get_watermark_info(session_key).await {
            events.push(gasket_types::events::ChatEvent::WatermarkInfo {
                watermark: info.watermark,
                max_sequence: info.max_sequence,
                uncompacted_count: info.uncompacted_count,
                compacted_percent: info.compacted_percent,
            });
        }
        Ok(events)
    }
}
