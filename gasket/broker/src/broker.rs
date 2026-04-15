//! Broker trait, Subscriber enum, and QueueMetrics.

use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Receiver;

use crate::error::BrokerError;
use crate::types::Envelope;

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
