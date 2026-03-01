//! Persistence hook — decouples session/memory I/O from the agent loop.
//!
//! Wraps [`SessionManager`] and [`MemoryStore`] so the agent loop itself
//! no longer needs to hold those dependencies.

use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::warn;

use crate::agent::memory::MemoryStore;
use crate::session::SessionManager;

use super::{AgentHook, ContextPrepareContext, SessionLoadContext, SessionSaveContext};

/// Default hook that persists conversation messages to SQLite.
///
/// Registered automatically by [`AgentLoop::new()`] to maintain backward
/// compatibility.
pub struct PersistenceHook {
    sessions: SessionManager,
    memory: MemoryStore,
    /// Track active sessions so we can save across on_session_load / on_session_save.
    active_sessions: Arc<Mutex<std::collections::HashMap<String, crate::session::Session>>>,
}

impl PersistenceHook {
    /// Create a new persistence hook.
    pub fn new(sessions: SessionManager, memory: MemoryStore) -> Self {
        Self {
            sessions,
            memory,
            active_sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
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
            let history = session.get_history(ctx.memory_window);
            ctx.history = history;

            // Cache the session for later save operations
            self.active_sessions
                .lock()
                .await
                .insert(ctx.session_key.clone(), session);
        }
    }

    async fn on_session_save(&self, ctx: &mut SessionSaveContext) {
        let mut sessions = self.active_sessions.lock().await;
        let session = match sessions.get_mut(&ctx.session_key) {
            Some(s) => s,
            None => {
                // Session not in cache — load it
                let s = self.sessions.get_or_create(&ctx.session_key).await;
                sessions.insert(ctx.session_key.clone(), s);
                sessions.get_mut(&ctx.session_key).unwrap()
            }
        };

        if let Err(e) = self
            .sessions
            .append_message(session, &ctx.role, &ctx.content, ctx.tools_used.clone())
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
