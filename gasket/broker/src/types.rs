//! Core message types for the broker.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Strongly-typed topic enum. Rejects stringly-typed routing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum Topic {
    #[default]
    Inbound,
    Outbound,
    SystemEvent,
    ToolCall(String),
    LlmRequest,
    Stream(String),
    CronTrigger,
    Heartbeat,
    Custom(String),
}

impl Topic {
    /// Validate and create a ToolCall topic.
    /// Rejects empty tool names.
    pub fn tool_call(name: impl Into<String>) -> Result<Self, &'static str> {
        let name = name.into();
        if name.is_empty() {
            return Err("ToolCall topic name cannot be empty");
        }
        Ok(Topic::ToolCall(name))
    }

    /// Validate and create a Custom topic.
    /// Rejects empty custom names.
    pub fn custom(name: impl Into<String>) -> Result<Self, &'static str> {
        let name = name.into();
        if name.is_empty() {
            return Err("Custom topic name cannot be empty");
        }
        Ok(Topic::Custom(name))
    }

    pub fn delivery_mode(&self) -> DeliveryMode {
        match self {
            Topic::SystemEvent => DeliveryMode::Broadcast,
            _ => DeliveryMode::PointToPoint,
        }
    }
}

/// Compile-time decision per topic — not a runtime guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMode {
    /// Message consumed by exactly one subscriber (work-stealing)
    PointToPoint,
    /// Message delivered to all subscribers
    Broadcast,
}

/// Pure data envelope — no callbacks, no channels, fully Clone-safe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub id: uuid::Uuid,
    pub timestamp: u64,
    #[serde(skip, default)]
    pub topic: Topic,
    pub payload: serde_json::Value,
}

impl Envelope {
    /// Quick construction — auto-generates ID and timestamp.
    pub fn new(topic: Topic, payload: impl Serialize) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            topic,
            payload: serde_json::to_value(payload).unwrap_or(serde_json::Value::Null),
        }
    }
}

/// ACK result — only used in the Broker's side-channel, never serialized.
#[derive(Debug)]
pub enum AckResult {
    Ack,
    Nack(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_default_is_inbound() {
        assert_eq!(Topic::default(), Topic::Inbound);
    }

    #[test]
    fn topic_tool_call_valid() {
        assert!(Topic::tool_call("search").is_ok());
        assert_eq!(
            Topic::tool_call("search").unwrap(),
            Topic::ToolCall("search".into())
        );
    }

    #[test]
    fn topic_tool_call_rejects_empty() {
        assert!(Topic::tool_call("").is_err());
    }

    #[test]
    fn topic_custom_valid() {
        assert!(Topic::custom("my_topic").is_ok());
    }

    #[test]
    fn topic_custom_rejects_empty() {
        assert!(Topic::custom("").is_err());
    }

    #[test]
    fn delivery_mode_system_event_is_broadcast() {
        assert_eq!(Topic::SystemEvent.delivery_mode(), DeliveryMode::Broadcast);
    }

    #[test]
    fn delivery_mode_inbound_is_p2p() {
        assert_eq!(Topic::Inbound.delivery_mode(), DeliveryMode::PointToPoint);
    }

    #[test]
    fn delivery_mode_outbound_is_p2p() {
        assert_eq!(Topic::Outbound.delivery_mode(), DeliveryMode::PointToPoint);
    }

    #[test]
    fn envelope_new_generates_id_and_timestamp() {
        let env = Envelope::new(Topic::Inbound, serde_json::json!({"text": "hello"}));
        assert!(!env.id.is_nil());
        assert!(env.timestamp > 0);
        assert_eq!(env.topic, Topic::Inbound);
        assert_eq!(env.payload["text"], "hello");
    }

    #[test]
    fn envelope_is_clone_safe() {
        let env = Envelope::new(Topic::Outbound, serde_json::json!(42));
        let cloned = env.clone();
        assert_eq!(env.id, cloned.id);
        assert_eq!(env.payload, cloned.payload);
    }
}
