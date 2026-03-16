//! Index maintenance operations.

mod backup;
mod compact;
mod expire;
mod rebuild;
mod stats;

pub use backup::{backup_index, restore_index};
pub use compact::{compact_index, CompactionResult};
pub use expire::expire_documents;
pub use rebuild::{rebuild_index, RebuildResult};
pub use stats::IndexHealth;
