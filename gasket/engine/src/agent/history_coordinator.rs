//! History query coordinator — single entry point for agent loop history access.
//!
//! This module defines the unified query types that the agent loop uses
//! instead of directly calling EventStore, MemoryManager, and Compactor.

use chrono::{DateTime, Utc};
use gasket_storage::memory::{MemoryHit, MemoryQuery};
use gasket_types::session_event::SessionEvent;

use super::memory_manager::MemoryContext;

/// History query intent — the only query entry point type.
///
/// Each variant routes to a specific backend component:
/// - `SessionContext` → ContextCompactor (LSM-tree: L0 events + L1 summary)
/// - `LatestSummary` → ContextCompactor::load_summary()
/// - `SemanticSearch` → MemoryProvider::search()
/// - `MemoryContext` → MemoryProvider::load_for_context()
/// - `TimeRange` → EventStore::query()
#[derive(Debug)]
pub enum HistoryQuery {
    /// Get recent context for this session within a token budget.
    /// Routes to ContextCompactor.
    SessionContext {
        session_key: String,
        token_budget: usize,
    },
    /// Get the latest summary.
    /// Routes to ContextCompactor::load_summary().
    LatestSummary {
        session_key: String,
    },
    /// Cross-session semantic search.
    /// Routes to MemoryProvider::search().
    SemanticSearch {
        query: String,
        top_k: usize,
    },
    /// Three-phase memory loading.
    /// Routes to MemoryProvider::load_for_context().
    MemoryContext {
        query: MemoryQuery,
    },
    /// Raw events in a time range.
    /// Routes to EventStore::query().
    TimeRange {
        session_key: String,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
}

/// History query result.
#[derive(Debug)]
pub enum HistoryResult {
    /// Context messages with role info, ready for LLM consumption.
    Context(Vec<ContextMessage>),
    /// Summary from compactor.
    Summary(Option<String>),
    /// Memory hits from semantic search.
    Memories(Vec<MemoryHit>),
    /// Full memory context from three-phase loading.
    MemoryContext(MemoryContext),
    /// Raw events from event store.
    Events(Vec<SessionEvent>),
}

/// Context message with role information.
/// Maps to LLM ChatMessage — includes role for proper conversation structure.
#[derive(Debug, Clone)]
pub struct ContextMessage {
    pub role: String,    // "user" | "assistant" | "system"
    pub content: String,
}
