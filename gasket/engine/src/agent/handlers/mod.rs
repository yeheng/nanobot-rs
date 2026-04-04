//! Event handlers for the MaterializationEngine.

pub mod compaction_handler;
pub mod indexing_handler;
pub mod memory_update_handler;

pub use compaction_handler::CompactionHandler;
pub use indexing_handler::IndexingHandler;
pub use memory_update_handler::MemoryUpdateHandler;
