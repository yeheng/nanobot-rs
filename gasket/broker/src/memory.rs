//! In-memory broker using DashMap + async-channel (P2P) + tokio::broadcast (fanout).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;

use crate::broker::{QueueMetrics, Subscriber};
use crate::error::BrokerError;
use crate::types::{DeliveryMode, Envelope, Topic};

// ── Internal queue ─────────────────────────────────────────

enum QueueInner {
    PointToPoint {
        tx: async_channel::Sender<Envelope>,
        rx: async_channel::Receiver<Envelope>,
        stats: Arc<QueueStats>,
    },
    Broadcast {
        tx: tokio::sync::broadcast::Sender<Envelope>,
        stats: Arc<QueueStats>,
    },
}

struct QueueStats {
    published: AtomicU64,
    consumed: AtomicU64,
}

impl QueueStats {
    fn new() -> Self {
        Self {
            published: AtomicU64::new(0),
            consumed: AtomicU64::new(0),
        }
    }
}

// ── MemoryBroker ───────────────────────────────────────────

pub struct MemoryBroker {
    queues: DashMap<Topic, QueueInner>,
    p2p_capacity: usize,
    broadcast_capacity: usize,
}

impl std::fmt::Debug for MemoryBroker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryBroker")
            .field("topics", &self.queues.len())
            .field("p2p_capacity", &self.p2p_capacity)
            .field("broadcast_capacity", &self.broadcast_capacity)
            .finish()
    }
}

impl Default for MemoryBroker {
    fn default() -> Self {
        Self::new(1024, 256)
    }
}

impl MemoryBroker {
    pub fn new(p2p_capacity: usize, broadcast_capacity: usize) -> Self {
        Self {
            queues: DashMap::new(),
            p2p_capacity,
            broadcast_capacity,
        }
    }

    fn ensure_queue(&self, topic: &Topic) {
        self.queues
            .entry(topic.clone())
            .or_insert_with(|| match topic.delivery_mode() {
                DeliveryMode::PointToPoint => {
                    let (tx, rx) = async_channel::bounded(self.p2p_capacity);
                    QueueInner::PointToPoint {
                        tx,
                        rx,
                        stats: Arc::new(QueueStats::new()),
                    }
                }
                DeliveryMode::Broadcast => {
                    let (tx, _rx) = tokio::sync::broadcast::channel(self.broadcast_capacity);
                    QueueInner::Broadcast {
                        tx,
                        stats: Arc::new(QueueStats::new()),
                    }
                }
            });
    }

    /// Blocking publish — awaits when queue is full (natural backpressure).
    pub async fn publish(&self, envelope: Envelope) -> Result<(), BrokerError> {
        self.ensure_queue(&envelope.topic);

        let mut cq = self
            .queues
            .get_mut(&envelope.topic)
            .ok_or(BrokerError::Internal(
                "queue just created but not found".into(),
            ))?;

        match cq.value_mut() {
            QueueInner::PointToPoint { tx, stats, .. } => {
                tx.send(envelope)
                    .await
                    .map_err(|_| BrokerError::ChannelClosed)?;
                stats.published.fetch_add(1, Ordering::Relaxed);
            }
            QueueInner::Broadcast { tx, stats } => {
                let _ = tx.send(envelope);
                stats.published.fetch_add(1, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    /// Non-blocking publish — returns QueueFull immediately.
    pub fn try_publish(&self, envelope: Envelope) -> Result<(), BrokerError> {
        self.ensure_queue(&envelope.topic);

        let cq = self
            .queues
            .get(&envelope.topic)
            .ok_or(BrokerError::Internal(
                "queue just created but not found".into(),
            ))?;

        match cq.value() {
            QueueInner::PointToPoint { tx, stats, .. } => {
                tx.try_send(envelope).map_err(|e| match e {
                    async_channel::TrySendError::Full(_) => BrokerError::QueueFull,
                    async_channel::TrySendError::Closed(_) => BrokerError::ChannelClosed,
                })?;
                stats.published.fetch_add(1, Ordering::Relaxed);
            }
            QueueInner::Broadcast { tx, stats } => {
                let _ = tx.send(envelope);
                stats.published.fetch_add(1, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    /// Subscribe to a topic.
    pub async fn subscribe(&self, topic: &Topic) -> Result<Subscriber, BrokerError> {
        self.ensure_queue(topic);

        let mut cq = self.queues.get_mut(topic).ok_or(BrokerError::Internal(
            "queue just created but not found".into(),
        ))?;

        match cq.value_mut() {
            QueueInner::PointToPoint { rx, .. } => Ok(Subscriber::PointToPoint(rx.clone())),
            QueueInner::Broadcast { tx, .. } => Ok(Subscriber::Broadcast(tx.subscribe())),
        }
    }

    /// Close a topic's queue (graceful shutdown).
    pub async fn close_topic(&self, topic: &Topic) -> Result<(), BrokerError> {
        self.queues.remove(topic);
        Ok(())
    }

    /// Queue metrics snapshot.
    pub fn metrics(&self, topic: &Topic) -> Option<QueueMetrics> {
        self.queues.get(topic).map(|cq| match cq.value() {
            QueueInner::PointToPoint { tx, stats, .. } => QueueMetrics {
                depth: tx.len(),
                total_published: stats.published.load(Ordering::Relaxed),
                total_consumed: stats.consumed.load(Ordering::Relaxed),
            },
            QueueInner::Broadcast { tx, stats } => QueueMetrics {
                depth: tx.len(),
                total_published: stats.published.load(Ordering::Relaxed),
                total_consumed: stats.consumed.load(Ordering::Relaxed),
            },
        })
    }
}

// ── Tests ──────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use gasket_types::events::{ChannelType, InboundMessage, OutboundMessage};
    use std::time::Duration;

    fn dummy_inbound(content: &str) -> InboundMessage {
        InboundMessage {
            channel: ChannelType::Cli,
            sender_id: "test".into(),
            chat_id: "test".into(),
            content: content.into(),
            media: None,
            metadata: None,
            timestamp: Utc::now(),
            trace_id: None,
        }
    }

    #[tokio::test]
    async fn test_p2p_publish_and_subscribe() {
        let broker = MemoryBroker::default();
        let mut sub = broker.subscribe(&Topic::Inbound).await.unwrap();
        let env = Envelope::new(
            Topic::Inbound,
            crate::types::BrokerPayload::Inbound(dummy_inbound("hello")),
        );
        broker.publish(env).await.unwrap();
        let received = tokio::time::timeout(Duration::from_secs(1), sub.recv()).await;
        assert!(received.is_ok());
        assert!(matches!(
            received.unwrap().unwrap().payload.as_ref(),
            crate::types::BrokerPayload::Inbound(InboundMessage { content, .. }) if content == "hello"
        ));
    }

    #[tokio::test]
    async fn test_backpressure_try_publish() {
        let broker = MemoryBroker::new(10, 10);
        let _sub = broker.subscribe(&Topic::Inbound).await.unwrap();
        for i in 0..10 {
            let msg = dummy_inbound(&i.to_string());
            assert!(broker
                .try_publish(Envelope::new(
                    Topic::Inbound,
                    crate::types::BrokerPayload::Inbound(msg)
                ))
                .is_ok());
        }
        let env = Envelope::new(
            Topic::Inbound,
            crate::types::BrokerPayload::Inbound(dummy_inbound("overflow")),
        );
        assert!(matches!(
            broker.try_publish(env),
            Err(BrokerError::QueueFull)
        ));
    }

    #[tokio::test]
    async fn test_work_stealing_two_consumers() {
        let broker = MemoryBroker::new(100, 10);
        let mut sub1 = broker.subscribe(&Topic::Inbound).await.unwrap();
        let mut sub2 = broker.subscribe(&Topic::Inbound).await.unwrap();
        for i in 0..10 {
            broker
                .publish(Envelope::new(
                    Topic::Inbound,
                    crate::types::BrokerPayload::Inbound(dummy_inbound(&i.to_string())),
                ))
                .await
                .unwrap();
        }
        let mut total = 0;
        for _ in 0..10 {
            let result = tokio::select! {
                r1 = sub1.recv() => r1.ok(),
                r2 = sub2.recv() => r2.ok(),
            };
            if result.is_some() {
                total += 1;
            }
        }
        assert_eq!(total, 10);
    }

    #[tokio::test]
    async fn test_broadcast_both_subscribers_receive() {
        let broker = MemoryBroker::default();
        let mut sub1 = broker.subscribe(&Topic::SystemEvent).await.unwrap();
        let mut sub2 = broker.subscribe(&Topic::SystemEvent).await.unwrap();
        broker
            .publish(Envelope::new(
                Topic::SystemEvent,
                crate::types::BrokerPayload::Outbound(OutboundMessage::new(
                    ChannelType::Cli,
                    "broadcast",
                    "alert",
                )),
            ))
            .await
            .unwrap();
        let r1 = tokio::time::timeout(Duration::from_secs(1), sub1.recv())
            .await
            .unwrap()
            .unwrap();
        let r2 = tokio::time::timeout(Duration::from_secs(1), sub2.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r1.payload.as_ref(), r2.payload.as_ref());
    }

    #[tokio::test]
    async fn test_lagged_subscriber_detection() {
        let broker = MemoryBroker::new(100, 3);
        let mut fast_sub = broker.subscribe(&Topic::SystemEvent).await.unwrap();
        let mut slow_sub = broker.subscribe(&Topic::SystemEvent).await.unwrap();

        for i in 0..5 {
            broker
                .publish(Envelope::new(
                    Topic::SystemEvent,
                    crate::types::BrokerPayload::Outbound(OutboundMessage::new(
                        ChannelType::Cli,
                        "lag",
                        i.to_string(),
                    )),
                ))
                .await
                .unwrap();
        }

        // Fast subscriber drains
        for _ in 0..5 {
            let _ = fast_sub.recv().await;
        }

        // Slow subscriber should see Lagged
        let result = slow_sub.recv().await;
        assert!(matches!(result, Err(BrokerError::Lagged(_))));
    }

    #[tokio::test]
    async fn test_metrics_tracking() {
        let broker = MemoryBroker::default();
        let _sub = broker.subscribe(&Topic::Inbound).await.unwrap();
        for i in 0..5 {
            broker
                .publish(Envelope::new(
                    Topic::Inbound,
                    crate::types::BrokerPayload::Inbound(dummy_inbound(&i.to_string())),
                ))
                .await
                .unwrap();
        }
        let m = broker.metrics(&Topic::Inbound).unwrap();
        assert_eq!(m.total_published, 5);
    }

    #[tokio::test]
    async fn test_close_topic() {
        let broker = MemoryBroker::default();
        let _sub = broker
            .subscribe(&Topic::Custom("temp".into()))
            .await
            .unwrap();
        broker
            .close_topic(&Topic::Custom("temp".into()))
            .await
            .unwrap();
        assert!(broker.metrics(&Topic::Custom("temp".into())).is_none());
    }

    #[tokio::test]
    async fn test_subscribe_creates_nonexistent_topic() {
        let broker = MemoryBroker::default();
        let _sub = broker
            .subscribe(&Topic::Custom("new".into()))
            .await
            .unwrap();
        assert!(broker.metrics(&Topic::Custom("new".into())).is_some());
    }
}
