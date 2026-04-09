pub mod builder;
pub mod coordinator;
pub mod indexing;

// Re-exports
pub use builder::{build_default_hooks, BuildOutcome, ChatRequest, ContextBuilder};
pub use coordinator::ContextMessage;
pub use indexing::{IndexingQueue, IndexingService, Priority, QueueError};
