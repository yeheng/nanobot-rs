//! Broker trait, Subscriber enum, and QueueMetrics.

use async_trait::async_trait;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Receiver;

use crate::error::BrokerError;
use crate::types::{AckResult, Envelope, Topic};

/// Queue metrics snapshot.
#[derive(Debug, Clone, Default)]
pub struct QueueMetrics {
    pub depth: usize,
    pub total_published: u64,
    pub total_consumed: u64,
}

/// Unified receiver that hides the underlying channel type.
pub enum Subscriber {
    PointToPoint(async_channel::Receiver<Envelope>),
    Broadcast(Receiver<Envelope>),
}

impl Subscriber {
    pub async fn recv(&mut self) -> Result<Envelope, BrokerError> {
        match self {
            Subscriber::PointToPoint(rx) => rx.recv().await.map_err(|_| BrokerError::ChannelClosed),
            Subscriber::Broadcast(rx) => rx.recv().await.map_err(|e| match e {
                RecvError::Closed => BrokerError::ChannelClosed,
                RecvError::Lagged(n) => BrokerError::Lagged(n),
            }),
        }
    }
}

/// Core broker abstraction.
///
/// Note on `async_trait`: Rust 1.75+ supports native `async fn in trait`,
/// but we need `dyn MessageBroker` (object-safety), so we use `#[async_trait]`
/// for now. Migration to native async is a future optimization.
#[async_trait]
pub trait MessageBroker: Send + Sync {
    /// Blocking publish — awaits when queue is full (natural backpressure).
    async fn publish(&self, envelope: Envelope) -> Result<(), BrokerError>;

    /// Non-blocking publish — returns QueueFull immediately.
    fn try_publish(&self, envelope: Envelope) -> Result<(), BrokerError>;

    /// Publish with ACK — returns a receiver for the consumer's acknowledgment.
    async fn publish_with_ack(
        &self,
        envelope: Envelope,
    ) -> Result<tokio::sync::oneshot::Receiver<AckResult>, BrokerError>;

    /// Acknowledge a message by ID (consumer-side).
    fn ack(&self, id: uuid::Uuid) -> Result<(), BrokerError>;

    /// Negatively acknowledge a message by ID (consumer-side).
    fn nack(&self, id: uuid::Uuid, reason: String) -> Result<(), BrokerError>;

    /// Subscribe to a topic.
    async fn subscribe(&self, topic: &Topic) -> Result<Subscriber, BrokerError>;

    /// Close a topic's queue (graceful shutdown).
    async fn close_topic(&self, topic: &Topic) -> Result<(), BrokerError>;

    /// Queue metrics snapshot.
    fn metrics(&self, topic: &Topic) -> Option<QueueMetrics>;
}
