//! Message bus for inter-component communication

use tokio::sync::mpsc::{channel, Receiver, Sender};
use tracing::instrument;

use super::events::{InboundMessage, OutboundMessage};

/// Message bus for routing messages between channels and agent.
///
/// The bus owns only the sender halves (cloneable). Receivers are returned
/// separately from `new()` and should be moved directly to their consumers
/// — no Mutex, no Option, no Arc needed for the receive side.
#[derive(Clone)]
pub struct MessageBus {
    inbound_tx: Sender<InboundMessage>,
    outbound_tx: Sender<OutboundMessage>,
}

impl MessageBus {
    /// Create a new message bus, returning the bus (senders only) plus both receivers.
    ///
    /// The caller must move each `Receiver` to its single consumer at
    /// initialization time. This avoids wrapping receivers in `Arc<Mutex<Option<…>>>`.
    pub fn new(buffer_size: usize) -> (Self, Receiver<InboundMessage>, Receiver<OutboundMessage>) {
        let (inbound_tx, inbound_rx) = channel(buffer_size);
        let (outbound_tx, outbound_rx) = channel(buffer_size);

        (
            Self {
                inbound_tx,
                outbound_tx,
            },
            inbound_rx,
            outbound_rx,
        )
    }

    /// Publish an inbound message
    #[instrument(name = "bus.publish_inbound", skip_all)]
    pub async fn publish_inbound(&self, msg: InboundMessage) {
        if let Err(e) = self.inbound_tx.send(msg).await {
            tracing::error!("Failed to publish inbound message: {}", e);
        }
    }

    /// Publish an outbound message (routes to the Outbound Actor)
    #[instrument(name = "bus.publish_outbound", skip_all)]
    pub async fn publish_outbound(&self, msg: OutboundMessage) {
        if let Err(e) = self.outbound_tx.send(msg).await {
            tracing::error!("Failed to publish outbound message: {}", e);
        }
    }

    /// Get a cloneable sender for inbound messages
    pub fn inbound_sender(&self) -> Sender<InboundMessage> {
        self.inbound_tx.clone()
    }

    /// Get a cloneable sender for outbound messages
    pub fn outbound_sender(&self) -> Sender<OutboundMessage> {
        self.outbound_tx.clone()
    }
}

impl Default for MessageBus {
    fn default() -> Self {
        let (bus, _, _) = Self::new(100);
        bus
    }
}

/// Convenience type alias for the tuple returned by `MessageBus::new()`.
pub type MessageBusComponents = (
    MessageBus,
    Receiver<InboundMessage>,
    Receiver<OutboundMessage>,
);
