//! Persistence hook — decouples session/memory I/O from the agent loop.
//!
//! Wraps [`SessionManager`] and [`MemoryStore`] so the agent loop itself
//! no longer needs to hold those dependencies.
//!
//! **Stateless by design**: this hook holds no per-request mutable state.
//! All session I/O goes directly through SQLite via `SessionManager`.
//! Per-session request serialization in the gateway guarantees that
//! concurrent callers never race on the same session.

use tracing::warn;

use crate::agent::memory::MemoryStore;
use crate::session::SessionManager;

use super::{AgentHook, ContextPrepareContext, SessionLoadContext, SessionSaveContext};

/// Default hook that persists conversation messages to SQLite.
///
/// Registered automatically by [`AgentLoop::new()`] to maintain backward
/// compatibility.
///
/// # Stateless Contract
///
/// This hook does **not** maintain any in-memory session cache.  Every
/// read goes directly to SQLite (via `SessionManager::get_or_create`) and
/// every write uses the stateless `SessionManager::append_by_key`.  This
/// eliminates the clone-modify-overwrite race condition that existed when
/// an `active_sessions` HashMap was used as a cache.
pub struct PersistenceHook {
    sessions: SessionManager,
    memory: MemoryStore,
}

impl PersistenceHook {
    /// Create a new persistence hook.
    pub fn new(sessions: SessionManager, memory: MemoryStore) -> Self {
        Self { sessions, memory }
    }

    /// Get a reference to the underlying `SessionManager` (for clear_session, etc.).
    pub fn sessions(&self) -> &SessionManager {
        &self.sessions
    }

    /// Get a reference to the underlying `MemoryStore`.
    pub fn memory(&self) -> &MemoryStore {
        &self.memory
    }
}

#[async_trait::async_trait]
impl AgentHook for PersistenceHook {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn on_session_load(&self, ctx: &mut SessionLoadContext) {
        // Only populate if no other hook has already filled history
        if ctx.history.is_empty() {
            let session = self.sessions.get_or_create(&ctx.session_key).await;
            ctx.history = session.get_history(ctx.memory_window);
        }
    }

    async fn on_session_save(&self, ctx: &mut SessionSaveContext) {
        // Stateless write: directly append to SQLite without any in-memory cache
        if let Err(e) = self
            .sessions
            .append_by_key(&ctx.session_key, &ctx.role, &ctx.content, ctx.tools_used.clone())
            .await
        {
            warn!("Failed to persist {} message to SQLite: {}", ctx.role, e);
        }

        let history_entry = format!("{}: {}", capitalize_role(&ctx.role), &ctx.content);
        if let Err(e) = self.memory.append_history(&history_entry).await {
            warn!("Failed to persist {} history to SQLite: {}", ctx.role, e);
        }
    }

    async fn on_context_prepare(&self, ctx: &mut ContextPrepareContext) {
        // Provide long-term memory if no other hook has set it
        if ctx.memory.is_none() {
            ctx.memory = self.memory.read_long_term().await.ok();
        }
    }
}

fn capitalize_role(role: &str) -> &str {
    match role {
        "user" => "User",
        "assistant" => "Assistant",
        other => other,
    }
}
