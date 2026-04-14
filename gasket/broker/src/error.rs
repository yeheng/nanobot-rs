//! Error types for the message broker.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BrokerError {
    #[error("Queue is full for topic")]
    QueueFull,

    #[error("Channel closed")]
    ChannelClosed,

    #[error("Subscriber lagged behind by {0} messages")]
    Lagged(u64),

    #[error("Topic not found")]
    TopicNotFound,

    #[error("Invalid topic: {0}")]
    InvalidTopic(String),

    #[error("ACK channel already consumed for message {0}")]
    AckAlreadyConsumed(uuid::Uuid),

    #[error("Internal error: {0}")]
    Internal(String),
}
