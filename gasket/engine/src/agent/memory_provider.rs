//! MemoryProvider trait — query interface extracted from MemoryManager.
//!
//! This trait defines the narrow interface that HistoryCoordinator depends on,
//! decoupling the coordinator from the concrete MemoryManager implementation.

use anyhow::Result;
use async_trait::async_trait;
use gasket_storage::memory::{MemoryHit, MemoryQuery};
use gasket_types::session_event::SessionEvent;

use super::memory_manager::MemoryContext;

/// MemoryProvider trait — memory system query interface.
///
/// Extracted from MemoryManager to allow:
/// - HistoryCoordinator to depend on trait, not concrete type
/// - Testing with mock implementations
/// - Future alternative memory backends
#[async_trait]
pub trait MemoryProvider: Send + Sync {
    /// Three-phase loading (bootstrap/scenario/on-demand).
    async fn load_for_context(&self, query: &MemoryQuery) -> Result<MemoryContext>;

    /// Semantic search across memories.
    async fn search(&self, query: &str, top_k: usize) -> Result<Vec<MemoryHit>>;

    /// Extract knowledge from event (called by MemoryUpdateHandler).
    async fn update_from_event(&self, _event: &SessionEvent) -> Result<()> {
        // Default: no-op. Phase 3 MemoryUpdateHandler will provide real impl.
        Ok(())
    }
}
