//! L3 routing tests: handle_inbound short-circuits on pending, falls through otherwise.

use std::sync::Arc;

use gasket_engine::session::{HandleOutcome, PendingAskRegistryImpl};
use gasket_types::events::{ChannelType, SessionKey};
use gasket_types::pending_ask::PendingAskRegistry;

#[tokio::test]
async fn registry_try_fulfill_short_circuits_for_pending_session() {
    let registry = Arc::new(PendingAskRegistryImpl::new());
    let key = SessionKey::new(ChannelType::Cli, "a");

    let registration = registry
        .register(
            key.clone(),
            "q?".into(),
            std::time::Instant::now() + std::time::Duration::from_secs(60),
        )
        .unwrap();

    let msg = gasket_types::events::InboundMessage {
        channel: key.channel.clone(),
        sender_id: key.chat_id.clone(),
        chat_id: key.chat_id.clone(),
        content: "answer".into(),
        media: None,
        metadata: None,
        timestamp: chrono::Utc::now(),
        trace_id: None,
    };
    registry.try_fulfill(&key, msg).expect("fulfill");

    let answer = registration.answer_rx.await.unwrap();
    assert_eq!(answer.content, "answer");
}

#[tokio::test]
async fn registry_try_fulfill_misses_for_other_session() {
    let registry = Arc::new(PendingAskRegistryImpl::new());
    let key_a = SessionKey::new(ChannelType::Cli, "a");
    let key_b = SessionKey::new(ChannelType::Cli, "b");

    let _ra = registry
        .register(
            key_a.clone(),
            "q?".into(),
            std::time::Instant::now() + std::time::Duration::from_secs(60),
        )
        .unwrap();

    let msg = gasket_types::events::InboundMessage {
        channel: key_b.channel.clone(),
        sender_id: key_b.chat_id.clone(),
        chat_id: key_b.chat_id.clone(),
        content: "for-b".into(),
        media: None,
        metadata: None,
        timestamp: chrono::Utc::now(),
        trace_id: None,
    };
    let returned = registry.try_fulfill(&key_b, msg).unwrap_err();
    assert_eq!(returned.content, "for-b");
}

#[tokio::test]
async fn handle_outcome_enum_compiles() {
    fn _accept(out: HandleOutcome) {
        match out {
            HandleOutcome::Consumed => {}
            HandleOutcome::Replied(_) => {}
        }
    }
}
