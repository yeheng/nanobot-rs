//! Message bus for inter-component communication
//!
//! Three actors form a clean pipeline with zero locks:
//! Router → Session → Outbound

pub mod actors;
pub mod queue;

pub use actors::{
    run_outbound_actor, run_router_actor, run_session_actor, MessageHandler, StreamEvent,
};
pub use gasket_types::events::*;
pub use queue::MessageBus;

// Re-export gasket_types::events as a module for backward compatibility
pub mod events {
    pub use gasket_types::events::*;
}
