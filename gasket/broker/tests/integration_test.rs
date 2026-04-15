//! Integration tests: full pipeline + idle timeout + dead session respawn

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use gasket_broker::*;
use gasket_types::events::*;

struct EchoHandler;

#[async_trait]
impl session::MessageHandler for EchoHandler {
    async fn handle_message(
        &self,
        _: &SessionKey,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        Ok(format!("Echo: {}", message))
    }

    async fn handle_streaming_message(
        &self,
        _: &str,
        _: &SessionKey,
    ) -> Result<
        (
            tokio::sync::mpsc::Receiver<session::StreamEvent>,
            tokio::sync::oneshot::Receiver<
                Result<OutboundMessage, Box<dyn std::error::Error + Send + Sync>>,
            >,
        ),
        Box<dyn std::error::Error + Send + Sync>,
    > {
        unimplemented!()
    }
}

fn make_inbound(content: &str) -> InboundMessage {
    InboundMessage {
        channel: ChannelType::Cli,
        sender_id: "test".into(),
        chat_id: "test".into(),
        content: content.into(),
        media: None,
        metadata: None,
        timestamp: chrono::Utc::now(),
        trace_id: None,
    }
}

#[tokio::test]
async fn test_full_pipeline() {
    let broker: Arc<MemoryBroker> = Arc::new(MemoryBroker::new(100, 50));
    let handler = Arc::new(EchoHandler);
    let mgr = SessionManager::new(broker.clone(), handler, Duration::from_secs(60));
    tokio::spawn(mgr.run());

    let mut out_sub = broker.subscribe(&Topic::Outbound).await.unwrap();
    broker
        .publish(Envelope::new(
            Topic::Inbound,
            BrokerPayload::Inbound(make_inbound("Hello")),
        ))
        .await
        .unwrap();

    let env = tokio::time::timeout(Duration::from_secs(5), out_sub.recv())
        .await
        .unwrap()
        .unwrap();
    match env.payload.as_ref() {
        BrokerPayload::Outbound(msg) => assert_eq!(msg.content, "Echo: Hello"),
        _ => panic!("expected Outbound payload"),
    }
}

#[tokio::test]
async fn test_idle_timeout_gc() {
    let broker: Arc<MemoryBroker> = Arc::new(MemoryBroker::new(100, 50));
    let handler = Arc::new(EchoHandler);
    let mgr = SessionManager::new(broker.clone(), handler, Duration::from_millis(100));
    tokio::spawn(mgr.run());

    let mut out_sub = broker.subscribe(&Topic::Outbound).await.unwrap();
    broker
        .publish(Envelope::new(
            Topic::Inbound,
            BrokerPayload::Inbound(make_inbound("first")),
        ))
        .await
        .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(2), out_sub.recv()).await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    // After timeout, session should respawn on next message
    broker
        .publish(Envelope::new(
            Topic::Inbound,
            BrokerPayload::Inbound(make_inbound("after_gc")),
        ))
        .await
        .unwrap();
    let env = tokio::time::timeout(Duration::from_secs(2), out_sub.recv()).await;
    assert!(env.is_ok());
    match env.unwrap().unwrap().payload.as_ref() {
        BrokerPayload::Outbound(msg) => assert_eq!(msg.content, "Echo: after_gc"),
        _ => panic!("expected Outbound payload"),
    }
}

#[tokio::test]
async fn test_dead_session_respawn() {
    let broker: Arc<MemoryBroker> = Arc::new(MemoryBroker::new(100, 50));
    let handler = Arc::new(EchoHandler);
    let mgr = SessionManager::new(broker.clone(), handler, Duration::from_secs(60));
    tokio::spawn(mgr.run());

    let mut out_sub = broker.subscribe(&Topic::Outbound).await.unwrap();

    // First message creates a session
    let msg1 = make_inbound("msg1");
    let key = msg1.session_key().clone();
    broker
        .publish(Envelope::new(Topic::Inbound, BrokerPayload::Inbound(msg1)))
        .await
        .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(2), out_sub.recv()).await;

    // Second message should still work (respawn if dead, or reuse if alive)
    let msg2 = InboundMessage {
        channel: ChannelType::Cli,
        sender_id: "test".into(),
        chat_id: key.chat_id.clone(),
        content: "msg2".into(),
        media: None,
        metadata: None,
        timestamp: chrono::Utc::now(),
        trace_id: None,
    };
    broker
        .publish(Envelope::new(Topic::Inbound, BrokerPayload::Inbound(msg2)))
        .await
        .unwrap();
    let env = tokio::time::timeout(Duration::from_secs(2), out_sub.recv()).await;
    assert!(env.is_ok());
}
