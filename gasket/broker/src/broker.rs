//! Broker trait, Subscriber enum, and QueueMetrics.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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
    PointToPoint(async_channel::Receiver<Envelope>, Option<Arc<AtomicU64>>),
    Broadcast(Receiver<Envelope>, Option<Arc<AtomicU64>>),
}

impl Subscriber {
    pub fn p2p(rx: async_channel::Receiver<Envelope>) -> Self {
        Self::PointToPoint(rx, None)
    }

    pub fn broadcast(rx: Receiver<Envelope>) -> Self {
        Self::Broadcast(rx, None)
    }

    pub fn with_counter(mut self, counter: Arc<AtomicU64>) -> Self {
        match &mut self {
            Self::PointToPoint(_, c) | Self::Broadcast(_, c) => *c = Some(counter),
        }
        self
    }

    pub async fn recv(&mut self) -> Result<Envelope, BrokerError> {
        let result = match self {
            Subscriber::PointToPoint(rx, _) => {
                rx.recv().await.map_err(|_| BrokerError::ChannelClosed)
            }
            Subscriber::Broadcast(rx, _) => rx.recv().await.map_err(|e| match e {
                RecvError::Closed => BrokerError::ChannelClosed,
                RecvError::Lagged(n) => BrokerError::Lagged(n),
            }),
        };
        if result.is_ok() {
            let counter = match self {
                Subscriber::PointToPoint(_, c) => c,
                Subscriber::Broadcast(_, c) => c,
            };
            if let Some(c) = counter {
                c.fetch_add(1, Ordering::Relaxed);
            }
        }
        result
    }
}
