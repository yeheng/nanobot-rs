//! History query coordinator — single entry point for agent loop history access.
//!
//! This module defines the unified query types that the agent loop uses
//! instead of directly calling EventStore, MemoryManager, and Compactor.

use anyhow::Result;
use chrono::{DateTime, Utc};
use gasket_storage::memory::{MemoryHit, MemoryQuery};
use gasket_storage::{EventFilter, EventStoreTrait, SqliteStore};
use gasket_types::session_event::SessionEvent;
use std::sync::Arc;

use crate::session::compactor::ContextCompactor;
use crate::session::memory::MemoryContext;
use crate::session::store::MemoryProvider;

/// History query intent — the only query entry point type.
///
/// Each variant routes to a specific backend component:
/// - `SessionContext` → EventStore + token budget trimming
/// - `LatestSummary` → SqliteStore::load_session_summary() (watermark-based)
/// - `SemanticSearch` → MemoryProvider::search()
/// - `MemoryContext` → MemoryProvider::load_for_context()
/// - `TimeRange` → EventStore::query()
#[derive(Debug)]
pub enum HistoryQuery {
    /// Get recent context for this session within a token budget.
    SessionContext {
        session_key: String,
        token_budget: usize,
    },
    /// Get the latest summary with its sequence watermark.
    /// Routes to SqliteStore::load_session_summary().
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
    /// Summary from session_summaries table: (content, covered_upto_sequence).
    Summary(Option<(String, i64)>),
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
    event_store: Arc<dyn EventStoreTrait>,
    _compactor: Arc<ContextCompactor>,
    sqlite_store: Arc<SqliteStore>,
    memory: Arc<dyn MemoryProvider>,
}

impl HistoryCoordinator {
    pub fn new(
        event_store: Arc<dyn EventStoreTrait>,
        compactor: Arc<ContextCompactor>,
        sqlite_store: Arc<SqliteStore>,
        memory: Arc<dyn MemoryProvider>,
    ) -> Self {
        Self {
            event_store,
            _compactor: compactor,
            sqlite_store,
            memory,
        }
    }

    /// Unified query entry point
    pub async fn query(&self, query: HistoryQuery) -> Result<HistoryResult> {
        match query {
            HistoryQuery::SessionContext {
                session_key,
                token_budget,
            } => {
                let key = gasket_types::SessionKey::parse(&session_key).unwrap_or_else(|| {
                    gasket_types::SessionKey::new(gasket_types::ChannelType::Cli, &session_key)
                });
                let events = self
                    .event_store
                    .query_events(&EventFilter {
                        session_key: Some(key),

                        ..Default::default()
                    })
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
                    let role = event.event_type.role_str().to_string();
                    selected.push(ContextMessage {
                        role,
                        content: event.content,
                    });
                }
                selected.reverse();
                Ok(HistoryResult::Context(selected))
            }
            HistoryQuery::LatestSummary { session_key } => {
                // Use the dedicated session_summaries table (watermark-based)
                let key = gasket_types::SessionKey::parse(&session_key).unwrap_or_else(|| {
                    gasket_types::SessionKey::new(gasket_types::ChannelType::Cli, &session_key)
                });
                let summary = self
                    .sqlite_store
                    .load_session_summary(&key)
                    .await
                    .map_err(|e| anyhow::anyhow!("sqlite store error: {}", e))?;
                Ok(HistoryResult::Summary(summary))
            }
            HistoryQuery::SemanticSearch { query, top_k } => {
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
            HistoryQuery::TimeRange {
                session_key,
                start,
                end,
            } => {
                let key = gasket_types::SessionKey::parse(&session_key).unwrap_or_else(|| {
                    gasket_types::SessionKey::new(gasket_types::ChannelType::Cli, &session_key)
                });
                let events = self
                    .event_store
                    .query_events(&EventFilter {
                        session_key: Some(key),

                        time_range: Some((start, end)),
                        ..Default::default()
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("event store error: {}", e))?;
                Ok(HistoryResult::Events(events))
            }
        }
    }

    /// Save event — delegates to EventStoreTrait::append()
    pub async fn save_event(
        &self,
        event: &gasket_types::session_event::SessionEvent,
    ) -> Result<()> {
        self.event_store.append(event).await?;
        Ok(())
    }
}
