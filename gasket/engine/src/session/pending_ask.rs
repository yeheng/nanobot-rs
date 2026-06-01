//! Concrete `PendingAskRegistry` implementation backed by a `Mutex<HashMap>`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use tokio::sync::oneshot;

use gasket_types::events::{InboundMessage, SessionKey};
use gasket_types::pending_ask::{AskAnswer, AskError, AskRegistration, PendingAskRegistry};

/// Internal slot record.
struct Slot {
    ask_id: uuid::Uuid,
    answer_tx: oneshot::Sender<AskAnswer>,
    /// Stored for debugging/logging; not read by current fulfillment logic.
    #[allow(dead_code)]
    prompt: String,
    deadline: Instant,
}

impl Slot {
    /// True when the deadline has passed — the ask should be treated as expired.
    fn is_expired(&self) -> bool {
        Instant::now() > self.deadline
    }
}

/// In-memory `PendingAskRegistry`. Single slot per `SessionKey`.
pub struct PendingAskRegistryImpl {
    inner: Mutex<HashMap<SessionKey, Slot>>,
}

impl PendingAskRegistryImpl {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for PendingAskRegistryImpl {
    fn default() -> Self {
        Self::new()
    }
}

impl PendingAskRegistry for PendingAskRegistryImpl {
    fn register(
        &self,
        key: SessionKey,
        prompt: String,
        deadline: Instant,
    ) -> Result<AskRegistration, AskError> {
        let mut guard = self
            .inner
            .lock()
            .expect("PendingAskRegistry mutex poisoned");

        // Evict stale/expired slots so a new registration can succeed.
        if let Some(existing) = guard.get(&key) {
            if existing.answer_tx.is_closed() || existing.is_expired() {
                guard.remove(&key);
            }
        }

        if guard.contains_key(&key) {
            return Err(AskError::AlreadyPending(key));
        }

        let (answer_tx, answer_rx) = oneshot::channel::<AskAnswer>();
        let ask_id = uuid::Uuid::new_v4();

        guard.insert(
            key.clone(),
            Slot {
                ask_id,
                answer_tx,
                prompt,
                deadline,
            },
        );
        Ok(AskRegistration { ask_id, answer_rx })
    }

    fn cancel(&self, key: &SessionKey, ask_id: uuid::Uuid) {
        let mut guard = self
            .inner
            .lock()
            .expect("PendingAskRegistry mutex poisoned");
        if let Some(slot) = guard.get(key) {
            if slot.ask_id == ask_id {
                guard.remove(key);
            }
        }
    }

    fn try_fulfill(&self, key: &SessionKey, msg: InboundMessage) -> Result<(), InboundMessage> {
        let mut guard = self
            .inner
            .lock()
            .expect("PendingAskRegistry mutex poisoned");

        // Evict stale/expired slots before attempting to fulfill.
        if let Some(existing) = guard.get(key) {
            if existing.answer_tx.is_closed() || existing.is_expired() {
                guard.remove(key);
                return Err(msg);
            }
        }

        let Some(slot) = guard.remove(key) else {
            return Err(msg);
        };

        let answer = AskAnswer::from_inbound(msg);
        let _ = slot.answer_tx.send(answer);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gasket_types::events::ChannelType;
    use std::time::Duration;

    fn key(id: &str) -> SessionKey {
        SessionKey::new(ChannelType::Cli, id)
    }

    fn dummy_inbound(content: &str, key: &SessionKey) -> InboundMessage {
        InboundMessage {
            channel: key.channel.clone(),
            sender_id: "sender".to_string(),
            chat_id: key.chat_id.clone(),
            content: content.to_string(),
            media: None,
            metadata: None,
            timestamp: chrono::Utc::now(),
            trace_id: None,
        }
    }

    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    #[test]
    fn register_then_fulfill() {
        let reg = PendingAskRegistryImpl::new();
        let k = key("a");

        let registration = reg
            .register(k.clone(), "hello?".into(), deadline())
            .expect("register");

        let msg = dummy_inbound("yes", &k);
        reg.try_fulfill(&k, msg).expect("fulfill");

        let answer = registration
            .answer_rx
            .blocking_recv()
            .expect("receiver got answer");
        assert_eq!(answer.content, "yes");
        assert_eq!(answer.channel, ChannelType::Cli);
    }

    #[test]
    fn register_twice_same_session_rejected() {
        let reg = PendingAskRegistryImpl::new();
        let k = key("a");
        let _r1 = reg.register(k.clone(), "q1".into(), deadline()).unwrap();
        let err = reg
            .register(k.clone(), "q2".into(), deadline())
            .unwrap_err();
        assert!(matches!(err, AskError::AlreadyPending(_)));
    }

    #[test]
    fn register_two_different_sessions_independent() {
        let reg = PendingAskRegistryImpl::new();
        let ka = key("a");
        let kb = key("b");
        let ra = reg.register(ka.clone(), "qa".into(), deadline()).unwrap();
        let rb = reg.register(kb.clone(), "qb".into(), deadline()).unwrap();

        reg.try_fulfill(&kb, dummy_inbound("ans-b", &kb)).unwrap();
        let ans_b = rb.answer_rx.blocking_recv().unwrap();
        assert_eq!(ans_b.content, "ans-b");

        reg.try_fulfill(&ka, dummy_inbound("ans-a", &ka)).unwrap();
        let ans_a = ra.answer_rx.blocking_recv().unwrap();
        assert_eq!(ans_a.content, "ans-a");
    }

    #[test]
    fn cancel_clears_slot() {
        let reg = PendingAskRegistryImpl::new();
        let k = key("a");
        let r = reg.register(k.clone(), "q".into(), deadline()).unwrap();
        reg.cancel(&k, r.ask_id);
        let _r2 = reg
            .register(k.clone(), "q2".into(), deadline())
            .expect("re-register after cancel");
    }

    #[test]
    fn try_fulfill_no_pending_returns_msg() {
        let reg = PendingAskRegistryImpl::new();
        let k = key("a");
        let msg = dummy_inbound("hi", &k);
        let returned = reg.try_fulfill(&k, msg).unwrap_err();
        assert_eq!(returned.content, "hi");
    }

    #[test]
    fn fulfill_after_receiver_dropped_evicts_slot() {
        let reg = PendingAskRegistryImpl::new();
        let k = key("a");
        {
            let r = reg.register(k.clone(), "q".into(), deadline()).unwrap();
            drop(r);
        }
        let msg = dummy_inbound("hi", &k);
        let returned = reg.try_fulfill(&k, msg).unwrap_err();
        assert_eq!(returned.content, "hi");

        let _r2 = reg
            .register(k.clone(), "q2".into(), deadline())
            .expect("register after stale eviction");
    }

    #[test]
    fn register_evicts_stale_slot() {
        let reg = PendingAskRegistryImpl::new();
        let k = key("a");
        {
            let r = reg.register(k.clone(), "q".into(), deadline()).unwrap();
            drop(r);
        }
        let _r2 = reg
            .register(k.clone(), "q2".into(), deadline())
            .expect("register evicts stale slot");
    }

    #[test]
    fn try_fulfill_evicts_expired_slot() {
        let reg = PendingAskRegistryImpl::new();
        let k = key("a");
        // Deadline in the past — slot is immediately expired.
        let _r = reg
            .register(
                k.clone(),
                "q".into(),
                Instant::now() - Duration::from_secs(1),
            )
            .unwrap();

        let msg = dummy_inbound("answer", &k);
        let returned = reg.try_fulfill(&k, msg).unwrap_err();
        assert_eq!(returned.content, "answer");
    }

    #[test]
    fn register_evicts_expired_slot() {
        let reg = PendingAskRegistryImpl::new();
        let k = key("a");
        let _r1 = reg
            .register(
                k.clone(),
                "q".into(),
                Instant::now() - Duration::from_secs(1),
            )
            .unwrap();

        // Even though the receiver is still alive, the expired slot is evicted.
        let _r2 = reg
            .register(k.clone(), "q2".into(), deadline())
            .expect("register evicts expired slot");
    }
}
