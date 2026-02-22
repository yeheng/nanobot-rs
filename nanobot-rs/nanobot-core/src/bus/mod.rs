//! Message bus for inter-component communication

pub mod events;
pub mod queue;

pub use events::{
    ChannelType, InboundMessage, OutboundMessage,
    // Pre-defined channel constructors
    cli, dingtalk, discord, email, feishu, slack, telegram, wecom,
};
pub use queue::MessageBus;
