//! Message bus for inter-component communication

pub mod events;
pub mod queue;

pub use events::{
    // Pre-defined channel constructors
    cli,
    dingtalk,
    discord,
    email,
    feishu,
    slack,
    telegram,
    wecom,
    ChannelType,
    InboundMessage,
    OutboundMessage,
};
pub use queue::MessageBus;
