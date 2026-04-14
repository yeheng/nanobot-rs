//! gasket-broker: In-process topic-based message broker.
//!
//! Replaces the fixed two-channel `MessageBus` with a unified
//! topic-based routing system supporting ACK, backpressure,
//! and priority differentiation.
//!
//! **Dependency boundary:** This crate depends ONLY on `gasket-types`.
//! `OutboundDispatcher` lives in `gasket-engine` (needs channels crate).

pub mod broker;
pub mod error;
pub mod memory;
pub mod session;
pub mod types;

// OutboundDispatcher is in gasket-engine, NOT here (circular dep avoidance).
// See engine/src/broker_outbound.rs.

pub use broker::{MessageBroker, QueueMetrics, Subscriber};
pub use error::BrokerError;
pub use types::{AckResult, DeliveryMode, Envelope, Topic};

// Implemented in later tasks — uncomment as each is completed:
pub use memory::MemoryBroker;
// pub use session::SessionManager;
