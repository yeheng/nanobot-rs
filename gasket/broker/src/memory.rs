//! In-memory broker using DashMap + async-channel (P2P) + tokio::broadcast (fanout).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use tokio::sync::oneshot;

use crate::broker::{MessageBroker, QueueMetrics, Subscriber};
use crate::error::BrokerError;
use crate::types::{AckResult, DeliveryMode, Envelope, Topic};

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

// ── ACK side-channel ───────────────────────────────────────

struct AckTracker {
    pending: DashMap<uuid::Uuid, oneshot::Sender<AckResult>>,
}

impl AckTracker {
    fn new() -> Self {
        Self {
            pending: DashMap::new(),
        }
    }

    fn register(&self, id: uuid::Uuid, tx: oneshot::Sender<AckResult>) -> Result<(), BrokerError> {
        if self.pending.contains_key(&id) {
            return Err(BrokerError::AckAlreadyConsumed(id));
        }
        self.pending.insert(id, tx);
        Ok(())
    }

    fn resolve(&self, id: uuid::Uuid, result: AckResult) -> Result<(), BrokerError> {
        if let Some((_, tx)) = self.pending.remove(&id) {
            let _ = tx.send(result);
            Ok(())
        } else {
            Err(BrokerError::AckAlreadyConsumed(id))
        }
    }
}

// ── MemoryBroker ───────────────────────────────────────────

pub struct MemoryBroker {
    queues: DashMap<Topic, QueueInner>,
    ack_tracker: AckTracker,
    p2p_capacity: usize,
    broadcast_capacity: usize,
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
            ack_tracker: AckTracker::new(),
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
}

#[async_trait]
impl MessageBroker for MemoryBroker {
    async fn publish(&self, envelope: Envelope) -> Result<(), BrokerError> {
        self.ensure_queue(&envelope.topic);

        let mut cq = self
            .queues
            .get_mut(&envelope.topic)
            .ok_or(BrokerError::Internal("queue just created but not found".into()))?;

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

    fn try_publish(&self, envelope: Envelope) -> Result<(), BrokerError> {
        self.ensure_queue(&envelope.topic);

        let cq = self
            .queues
            .get(&envelope.topic)
            .ok_or(BrokerError::Internal("queue just created but not found".into()))?;

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

    async fn publish_with_ack(
        &self,
        envelope: Envelope,
    ) -> Result<oneshot::Receiver<AckResult>, BrokerError> {
        let id = envelope.id;
        let (ack_tx, ack_rx) = oneshot::channel();
        self.ack_tracker.register(id, ack_tx)?;
        self.publish(envelope).await?;
        Ok(ack_rx)
    }

    fn ack(&self, id: uuid::Uuid) -> Result<(), BrokerError> {
        self.ack_tracker.resolve(id, AckResult::Ack)
    }

    fn nack(&self, id: uuid::Uuid, reason: String) -> Result<(), BrokerError> {
        self.ack_tracker.resolve(id, AckResult::Nack(reason))
    }

    async fn subscribe(&self, topic: &Topic) -> Result<Subscriber, BrokerError> {
        self.ensure_queue(topic);

        let mut cq = self
            .queues
            .get_mut(topic)
            .ok_or(BrokerError::Internal("queue just created but not found".into()))?;

        match cq.value_mut() {
            QueueInner::PointToPoint { rx, .. } => Ok(Subscriber::PointToPoint(rx.clone())),
            QueueInner::Broadcast { tx, .. } => Ok(Subscriber::Broadcast(tx.subscribe())),
        }
    }

    async fn close_topic(&self, topic: &Topic) -> Result<(), BrokerError> {
        self.queues.remove(topic);
        Ok(())
    }

    fn metrics(&self, topic: &Topic) -> Option<QueueMetrics> {
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
    use std::time::Duration;

    #[tokio::test]
    async fn test_p2p_publish_and_subscribe() {
        let broker = MemoryBroker::default();
        let mut sub = broker.subscribe(&Topic::Inbound).await.unwrap();
        let env = Envelope::new(Topic::Inbound, serde_json::json!({"msg": "hello"}));
        broker.publish(env).await.unwrap();
        let received = tokio::time::timeout(Duration::from_secs(1), sub.recv()).await;
        assert!(received.is_ok());
        assert_eq!(received.unwrap().unwrap().payload["msg"], "hello");
    }

    #[tokio::test]
    async fn test_backpressure_try_publish() {
        let broker = MemoryBroker::new(10, 10);
        let _sub = broker.subscribe(&Topic::Inbound).await.unwrap();
        for i in 0..10 {
            assert!(
                broker
                    .try_publish(Envelope::new(Topic::Inbound, serde_json::json!(i)))
                    .is_ok()
            );
        }
        let env = Envelope::new(Topic::Inbound, serde_json::json!("overflow"));
        assert!(matches!(broker.try_publish(env), Err(BrokerError::QueueFull)));
    }

    #[tokio::test]
    async fn test_work_stealing_two_consumers() {
        let broker = MemoryBroker::new(100, 10);
        let mut sub1 = broker.subscribe(&Topic::Inbound).await.unwrap();
        let mut sub2 = broker.subscribe(&Topic::Inbound).await.unwrap();
        for i in 0..10 {
            broker
                .publish(Envelope::new(Topic::Inbound, serde_json::json!(i)))
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
                serde_json::json!("alert"),
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
        assert_eq!(r1.payload, r2.payload);
    }

    #[tokio::test]
    async fn test_lagged_subscriber_detection() {
        let broker = MemoryBroker::new(100, 3);
        let mut fast_sub = broker.subscribe(&Topic::SystemEvent).await.unwrap();
        let mut slow_sub = broker.subscribe(&Topic::SystemEvent).await.unwrap();

        for i in 0..5 {
            broker
                .publish(Envelope::new(Topic::SystemEvent, serde_json::json!(i)))
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
    async fn test_ack_round_trip() {
        let broker = MemoryBroker::default();
        let mut sub = broker.subscribe(&Topic::Inbound).await.unwrap();
        let env = Envelope::new(Topic::Inbound, serde_json::json!("important"));
        let mut ack_rx = broker.publish_with_ack(env).await.unwrap();
        let received = sub.recv().await.unwrap();
        broker.ack(received.id).unwrap();
        let ack_result = tokio::time::timeout(Duration::from_secs(1), &mut ack_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(ack_result, AckResult::Ack));
    }

    #[tokio::test]
    async fn test_nack_round_trip() {
        let broker = MemoryBroker::default();
        let mut sub = broker.subscribe(&Topic::Inbound).await.unwrap();
        let env = Envelope::new(Topic::Inbound, serde_json::json!("fail"));
        let mut ack_rx = broker.publish_with_ack(env).await.unwrap();
        let received = sub.recv().await.unwrap();
        broker
            .nack(received.id, "processing failed".into())
            .unwrap();
        let ack_result = tokio::time::timeout(Duration::from_secs(1), &mut ack_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(ack_result, AckResult::Nack(_)));
    }

    #[tokio::test]
    async fn test_metrics_tracking() {
        let broker = MemoryBroker::default();
        let _sub = broker.subscribe(&Topic::Inbound).await.unwrap();
        for i in 0..5 {
            broker
                .publish(Envelope::new(Topic::Inbound, serde_json::json!(i)))
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
