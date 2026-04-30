//! Shared types and events for gasket.
//!
//! This crate provides the core data types used across all gasket components:
//! - Message types (InboundMessage, OutboundMessage)
//! - Channel identifiers (ChannelType, SessionKey)
//! - WebSocket streaming messages
//! - Tool trait and base types
//! - Token tracking and budget enforcement
//!
//! By keeping these types in a separate crate, we avoid circular dependencies
//! between `gasket-core` and other crates.

pub mod events;
pub mod session_event;
pub mod token_tracker;
pub mod tool;

pub use events::{
    parse_phase_command, ChannelType, InboundMessage, MediaAttachment, OutboundMessage,
    OutboundPayload, SessionKey, SessionKeyParseError, StreamEvent, StreamEventKind, Target,
    WebSocketMessage,
};
pub use session_event::{
    EventMetadata, EventType, Session, SessionEvent, SessionMetadata, SummaryType,
};
pub use token_tracker::{
    calculate_cost, format_cost, format_token_usage, ModelPricing, SessionTokenStats, TokenTracker,
    TokenUsage,
};
pub use tool::{
    simple_schema, ApprovalCallback, NoopSpawner, SubagentResponse, SubagentResult,
    SubagentSpawner, SynthesisCallback, Tool, ToolApprovalRequest, ToolApprovalResponse,
    ToolContext, ToolControlSignal, ToolError, ToolMetadata, ToolOutput, ToolResult,
};
