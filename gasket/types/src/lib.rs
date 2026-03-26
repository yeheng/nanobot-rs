//! Shared types and events for gasket.
//!
//! This crate provides the core data types used across all gasket components:
//! - Message types (InboundMessage, OutboundMessage)
//! - Channel identifiers (ChannelType, SessionKey)
//! - WebSocket streaming messages
//! - Tool trait and base types
//!
//! By keeping these types in a separate crate, we avoid circular dependencies
//! between `gasket-core` and other crates.

pub mod events;
pub mod session_event;
pub mod tool;

pub use events::{
    ChannelType, InboundMessage, MediaAttachment, OutboundMessage, SessionKey,
    SessionKeyParseError, WebSocketMessage,
};
pub use session_event::{
    EventMetadata, EventType, EventTypeCategory, Session, SessionEvent, SessionMetadata,
    SummaryType, TokenUsage,
};
pub use tool::{simple_schema, Tool, ToolContext, ToolError, ToolMetadata, ToolResult};
