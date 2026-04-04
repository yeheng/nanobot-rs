//! History query coordinator — single entry point for agent loop history access.
//!
//! This module defines the unified query types that the agent loop uses
//! instead of directly calling EventStore, MemoryManager, and Compactor.

use std::sync::Arc;
use anyhow::Result;
use chrono::{DateTime, Utc};
use gasket_storage::EventStore;
use gasket_storage::memory::{MemoryHit, MemoryQuery};
use gasket_types::session_event::SessionEvent;

use super::compactor::ContextCompactor;
use super::memory_manager::MemoryContext;
use super::memory_manager::MemoryManager;

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
    LatestSummary { session_key: String },
    /// Cross-session semantic search.
    /// Routes to MemoryProvider::search().
    SemanticSearch { query: String, top_k: usize },
    /// Three-phase memory loading.
    /// Routes to MemoryProvider::load_for_context().
    MemoryContext { query: MemoryQuery },
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
    pub role: String, // "user" | "assistant" | "system"
    pub content: String,
}

/// History query coordinator — thin router, NOT a processor.
///
/// Routes each HistoryQuery variant to the optimal backend component.
/// Allows simple type transformations (SessionEvent → ContextMessage)
/// but contains NO business logic.
pub struct HistoryCoordinator {
    event_store: Arc<EventStore>,
    compactor: Arc<ContextCompactor>,
    memory: Arc<MemoryManager>,
}

impl HistoryCoordinator {
    pub fn new(
        event_store: Arc<EventStore>,
        compactor: Arc<ContextCompactor>,
        memory: Arc<MemoryManager>,
    ) -> Self {
        Self { event_store, compactor, memory }
    }

    /// Unified query entry point
    pub async fn query(&self, query: HistoryQuery) -> Result<HistoryResult> {
        match query {
            HistoryQuery::SessionContext { session_key, token_budget } => {
                // Phase 1: delegate to existing get_branch_history + trim
                let events = self.event_store
                    .get_branch_history(&session_key, "main")
                    .await
                    .map_err(|e| anyhow::anyhow!("event store error: {}", e))?;

                let mut selected = Vec::new();
                let mut tokens_used = 0;
                for event in events.into_iter().rev() {
                    let event_tokens = event.metadata.content_token_len;
                    if tokens_used + event_tokens > token_budget {
                        break;
                    }
                    tokens_used += event_tokens;
                    let role = match event.event_type {
                        gasket_types::session_event::EventType::UserMessage => "user".to_string(),
                        gasket_types::session_event::EventType::AssistantMessage => "assistant".to_string(),
                        _ => "system".to_string(),
                    };
                    selected.push(ContextMessage { role, content: event.content });
                }
                selected.reverse();
                Ok(HistoryResult::Context(selected))
            }
            HistoryQuery::LatestSummary { session_key } => {
                let summary = self.event_store
                    .get_latest_summary(&session_key, "main")
                    .await
                    .map_err(|e| anyhow::anyhow!("event store error: {}", e))?;
                Ok(HistoryResult::Summary(summary.map(|e| e.content)))
            }
            HistoryQuery::SemanticSearch { query, top_k } => {
                // MemoryManager doesn't expose public search().
                // Use load_for_context with text-based MemoryQuery.
                let memory_query = MemoryQuery {
                    text: Some(query),
                    tags: vec![],
                    scenario: None,
                    max_tokens: Some(top_k * 200),
                };
                let ctx = self.memory.load_for_context(&memory_query).await?;
                Ok(HistoryResult::MemoryContext(ctx))
            }
            HistoryQuery::MemoryContext { query } => {
                let ctx = self.memory.load_for_context(&query).await?;
                Ok(HistoryResult::MemoryContext(ctx))
            }
            HistoryQuery::TimeRange { session_key, start, end } => {
                let events = self.event_store
                    .get_branch_history(&session_key, "main")
                    .await
                    .map_err(|e| anyhow::anyhow!("event store error: {}", e))?;
                let filtered: Vec<_> = events
                    .into_iter()
                    .filter(|e| e.created_at >= start && e.created_at <= end)
                    .collect();
                Ok(HistoryResult::Events(filtered))
            }
        }
    }

    /// Save event — delegates to EventStore
    pub async fn save_event(
        &self,
        event: &gasket_types::session_event::SessionEvent,
    ) -> Result<()> {
        self.event_store.append_event(event).await
            .map_err(|e| anyhow::anyhow!("event store error: {}", e))
    }
}
