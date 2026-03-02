//! Storage traits for AgentLoop dependencies.
//!
//! Provides trait-based abstraction over session and memory storage,
//! enabling Null Object pattern for subagents that don't need persistence.

use std::sync::Arc;

use async_trait::async_trait;

use crate::session::{Session, SessionMessage};

/// Trait for session persistence operations.
///
/// Implemented by `SessionManager` for real persistence,
/// and `NoopSessionStorage` for subagents that don't need history.
#[async_trait]
pub trait SessionStorage: Send + Sync {
    /// Get or create a session by key.
    async fn get_or_create(&self, key: &str) -> Session;

    /// Save a session (after clear, etc.).
    async fn save(&self, session: &Session);

    /// Clear a session by key.
    async fn clear_session(&self, key: &str);

    /// Append a message to a session by key (stateless).
    async fn append_by_key(
        &self,
        session_key: &str,
        role: &str,
        content: &str,
        tools_used: Option<Vec<String>>,
    ) -> anyhow::Result<()>;
}

/// Trait for long-term memory operations.
///
/// Implemented by `MemoryStore` for real storage,
/// and `NoopLongTermMemory` for subagents that don't need MEMORY.md.
#[async_trait]
pub trait LongTermMemory: Send + Sync {
    /// Read long-term memory content.
    async fn read_long_term(&self) -> anyhow::Result<String>;
}

// ── Null Implementations ─────────────────────────────────────

/// No-op session storage for subagents.
///
/// Returns empty sessions, discards all writes.
/// Eliminates `Option<SessionManager>` checks in AgentLoop.
pub struct NoopSessionStorage;

#[async_trait]
impl SessionStorage for NoopSessionStorage {
    async fn get_or_create(&self, key: &str) -> Session {
        Session::new(key)
    }

    async fn save(&self, _session: &Session) {
        // No-op: subagents don't persist sessions
    }

    async fn clear_session(&self, _key: &str) {
        // No-op: nothing to clear
    }

    async fn append_by_key(
        &self,
        _session_key: &str,
        _role: &str,
        _content: &str,
        _tools_used: Option<Vec<String>>,
    ) -> anyhow::Result<()> {
        // No-op: subagents don't persist messages
        Ok(())
    }
}

/// No-op long-term memory for subagents.
///
/// Always returns empty string, discards all writes.
/// Eliminates `Option<MemoryStore>` checks in AgentLoop.
pub struct NoopLongTermMemory;

#[async_trait]
impl LongTermMemory for NoopLongTermMemory {
    async fn read_long_term(&self) -> anyhow::Result<String> {
        Ok(String::new())
    }
}

// ── Real Implementations (wrappers) ───────────────────────────

use crate::agent::MemoryStore;
use crate::session::SessionManager;

/// Real session storage backed by SessionManager.
pub struct RealSessionStorage(pub SessionManager);

#[async_trait]
impl SessionStorage for RealSessionStorage {
    async fn get_or_create(&self, key: &str) -> Session {
        self.0.get_or_create(key).await
    }

    async fn save(&self, session: &Session) {
        self.0.save(session).await
    }

    async fn clear_session(&self, key: &str) {
        self.0.clear_session(key).await
    }

    async fn append_by_key(
        &self,
        session_key: &str,
        role: &str,
        content: &str,
        tools_used: Option<Vec<String>>,
    ) -> anyhow::Result<()> {
        self.0
            .append_by_key(session_key, role, content, tools_used)
            .await
    }
}

/// Real long-term memory backed by MemoryStore.
pub struct RealLongTermMemory(pub MemoryStore);

#[async_trait]
impl LongTermMemory for RealLongTermMemory {
    async fn read_long_term(&self) -> anyhow::Result<String> {
        self.0.read_long_term().await
    }
}

// ── Context Compression Hook Null Implementation ─────────────

use super::summarization::ContextCompressionHook;

/// No-op context compression for subagents.
///
/// Always returns None (no summary).
/// Eliminates `Option<SummarizationService>` checks in AgentLoop.
pub struct NoopContextCompression;

#[async_trait]
impl ContextCompressionHook for NoopContextCompression {
    async fn compress(
        &self,
        _session_key: &str,
        _evicted_messages: &[SessionMessage],
    ) -> anyhow::Result<Option<String>> {
        Ok(None)
    }
}

// ── Helper constructors ───────────────────────────────────────

/// Create the standard storage stack for main agents.
pub fn real_storage_stack(
    memory: MemoryStore,
    sessions: SessionManager,
) -> (Arc<dyn SessionStorage>, Arc<dyn LongTermMemory>) {
    (
        Arc::new(RealSessionStorage(sessions)),
        Arc::new(RealLongTermMemory(memory)),
    )
}

/// Create the no-op storage stack for subagents.
pub fn noop_storage_stack() -> (Arc<dyn SessionStorage>, Arc<dyn LongTermMemory>) {
    (Arc::new(NoopSessionStorage), Arc::new(NoopLongTermMemory))
}
