//! OutboundDispatcher — replaces the Outbound Actor.
//!
//! Subscribes to `Topic::Outbound` via the broker and dispatches each
//! message to the appropriate IM provider. Each send is fire-and-forget
//! (`tokio::spawn`) to avoid Head-of-Line Blocking.

use std::sync::Arc;

use gasket_broker::{BrokerPayload, MemoryBroker, Topic};
use gasket_channels::provider::ImProviders;

/// Dispatches outbound messages from the broker to `ImProviders`.
///
/// Replaces `run_outbound_actor` from the old bus architecture.
/// The broker's topic-based routing means this is a pure consumer —
/// no inbound routing logic lives here.
pub struct OutboundDispatcher {
    broker: Arc<MemoryBroker>,
    providers: Arc<ImProviders>,
    #[cfg(feature = "webhook")]
    websocket_manager: Option<Arc<gasket_channels::websocket::WebSocketManager>>,
}

impl OutboundDispatcher {
    /// Create a new dispatcher without WebSocket support.
    pub fn new(broker: Arc<MemoryBroker>, providers: Arc<ImProviders>) -> Self {
        Self {
            broker,
            providers,
            #[cfg(feature = "webhook")]
            websocket_manager: None,
        }
    }

    /// Create a dispatcher with WebSocket manager support.
    #[cfg(feature = "webhook")]
    pub fn with_websocket(
        broker: Arc<MemoryBroker>,
        providers: Arc<ImProviders>,
        websocket_manager: Arc<gasket_channels::websocket::WebSocketManager>,
    ) -> Self {
        Self {
            broker,
            providers,
            websocket_manager: Some(websocket_manager),
        }
    }

    /// Main loop — subscribes to Outbound topic and dispatches.
    pub async fn run(self) {
        tracing::info!("OutboundDispatcher started");
        let mut sub = match self.broker.subscribe(&Topic::Outbound).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("OutboundDispatcher: subscribe failed: {}", e);
                return;
            }
        };

        while let Ok(envelope) = sub.recv().await {
            match envelope.payload.as_ref() {
                BrokerPayload::Outbound(msg) => {
                    // WebSocket messages go through WebSocketManager, not the providers enum
                    #[cfg(feature = "webhook")]
                    if let gasket_types::events::ChannelType::WebSocket = msg.channel {
                        if let Some(ref manager) = self.websocket_manager {
                            manager.send(msg.clone()).await;
                        } else {
                            tracing::warn!(
                                "OutboundDispatcher: websocket_manager is None, dropping WS message"
                            );
                        }
                        continue;
                    }

                    let providers = self.providers.clone();
                    let msg = msg.clone();
                    tokio::spawn(async move {
                        if let Err(e) = providers.send(&msg).await {
                            tracing::error!("Outbound delivery failed: {}", e);
                        }
                    });
                }
                other => {
                    tracing::warn!(
                        "OutboundDispatcher: unexpected payload on Outbound topic: {:?}",
                        other
                    );
                }
            }
        }
        tracing::info!("OutboundDispatcher shutting down");
    }
}
