//! Agent context trait for abstracting state management.
//!
//! This module provides a trait-based abstraction for agent state management,
//! eliminating `Option<T>` checks in the core loop.
//!
//! # Design
//!
//! - `AgentContext` trait defines the interface for session and memory operations
//! - `PersistentContext` provides full persistence (main agents)
//! - `StatelessContext` provides no-op implementations (subagents)
//!
//! # Example
//!
//! ```ignore
//! // Main agent with persistence
//! let context = PersistentContext::new(session_manager, summarization);
//!
//! // Subagent without persistence
//! let context = StatelessContext::new();
//!
//! // Use through trait
//! context.save_message(&session_key, "user", "Hello").await;
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use tracing::{debug, warn};

use crate::agent::summarization::SummarizationService;
use crate::bus::events::SessionKey;
use crate::session::{Session, SessionManager, SessionMessage};

/// Trait for agent context operations.
///
/// This abstracts session persistence and context compression,
/// allowing the agent loop to work without `Option<T>` checks.
#[async_trait]
pub trait AgentContext: Send + Sync {
    /// Load or create a session for the given key.
    async fn load_session(&self, key: &SessionKey) -> Session;

    /// Save a message to the session.
    ///
    /// # Errors
    ///
    /// Returns an error if the message fails to persist to storage.
    async fn save_message(
        &self,
        key: &SessionKey,
        role: &str,
        content: &str,
        tools: Option<Vec<String>>,
    ) -> Result<(), crate::error::AgentError>;

    /// Load an existing summary for the session.
    async fn load_summary(&self, key: &str) -> Option<String>;

    /// Compress context in the background (non-blocking).
    /// The implementation may spawn a background task or be a no-op.
    fn compress_context(&self, key: &str, evicted: &[SessionMessage]);

    /// Recall relevant historical messages based on semantic similarity.
    ///
    /// Returns the top-K most relevant messages from the embedding store.
    /// Returns an empty vector if semantic recall is not available.
    async fn recall_history(
        &self,
        _key: &str,
        _query_embedding: &[f32],
        _top_k: usize,
    ) -> anyhow::Result<Vec<String>> {
        // Default: no semantic recall available
        Ok(Vec::new())
    }

    /// Check if this context has persistence enabled.
    fn is_persistent(&self) -> bool;
}

/// Persistent context with full session and summarization support.
///
/// Used by main agents that need to persist conversations and compress context.
///
/// # Concurrency Safety
///
/// The `compression_in_progress` map ensures that only one summarization task
/// runs per session at any time. This prevents API flooding when multiple
/// messages trigger compression in quick succession.
pub struct PersistentContext {
    session_manager: Arc<SessionManager>,
    summarization: Arc<SummarizationService>,
    /// Tracks which sessions have an active compression task.
    /// Key: session_key, Value: flag indicating compression is in progress.
    compression_in_progress: Arc<DashMap<String, Arc<AtomicBool>>>,
}

impl PersistentContext {
    /// Create a new persistent context.
    pub fn new(
        session_manager: Arc<SessionManager>,
        summarization: Arc<SummarizationService>,
    ) -> Self {
        Self {
            session_manager,
            summarization,
            compression_in_progress: Arc::new(DashMap::new()),
        }
    }
}

#[async_trait]
impl AgentContext for PersistentContext {
    async fn load_session(&self, key: &SessionKey) -> Session {
        self.session_manager.get_or_create(key).await
    }

    async fn save_message(
        &self,
        key: &SessionKey,
        role: &str,
        content: &str,
        tools: Option<Vec<String>>,
    ) -> Result<(), crate::error::AgentError> {
        self.session_manager
            .append_by_key(key, role, content, tools)
            .await
            .map_err(|e| {
                crate::error::AgentError::Other(format!("Failed to persist message: {}", e))
            })
    }

    async fn load_summary(&self, key: &str) -> Option<String> {
        self.summarization.load_summary(key).await
    }

    fn compress_context(&self, key: &str, evicted: &[SessionMessage]) {
        if evicted.is_empty() {
            return;
        }

        // Get or create the in-progress flag for this session
        let flag = self
            .compression_in_progress
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone();

        // Try to acquire the "lock" via compare-and-swap
        // If already true, another task is running - skip this one
        if flag
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            debug!(
                "[Summarization] Skipping compression for session '{}' - another task is already running",
                key
            );
            return;
        }

        let svc = Arc::clone(&self.summarization);
        let key_owned = key.to_string();
        let evicted = evicted.to_vec();
        let flag_clone = flag.clone();
        let compression_map = Arc::clone(&self.compression_in_progress);

        tokio::spawn(async move {
            debug!(
                "[Summarization] Background compression task started for session '{}'",
                key_owned
            );
            match svc.compress(&key_owned, &evicted).await {
                Ok(_) => {
                    debug!(
                        "[Summarization] Background compression completed for session '{}'",
                        key_owned
                    );
                }
                Err(e) => {
                    warn!(
                        "[Summarization] Background compression failed for session '{}': {}",
                        key_owned, e
                    );
                }
            }

            // Release the "lock" and clean up the entry
            flag_clone.store(false, Ordering::Release);
            // Only remove if the flag is still the one we set (defensive)
            compression_map.remove_if(&key_owned, |_, v| Arc::ptr_eq(v, &flag_clone));
        });
    }

    async fn recall_history(
        &self,
        key: &str,
        query_embedding: &[f32],
        top_k: usize,
    ) -> anyhow::Result<Vec<String>> {
        let results = self
            .summarization
            .recall_history(key, query_embedding, top_k)
            .await;
        Ok(results
            .into_iter()
            .map(|(content, _score)| content)
            .collect())
    }

    fn is_persistent(&self) -> bool {
        true
    }
}

/// Stateless context with no persistence.
///
/// Used by subagents that don't need to persist conversations.
/// All operations are no-ops except for session creation (in-memory only).
pub struct StatelessContext;

impl StatelessContext {
    /// Create a new stateless context.
    pub fn new() -> Self {
        Self
    }
}

impl Default for StatelessContext {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentContext for StatelessContext {
    async fn load_session(&self, key: &SessionKey) -> Session {
        // Create an in-memory session without persistence
        Session::from_key(key.clone())
    }

    async fn save_message(
        &self,
        _key: &SessionKey,
        _role: &str,
        _content: &str,
        _tools: Option<Vec<String>>,
    ) -> Result<(), crate::error::AgentError> {
        // No-op for stateless context - always succeeds
        Ok(())
    }

    async fn load_summary(&self, _key: &str) -> Option<String> {
        // No summary for stateless context
        None
    }

    fn compress_context(&self, _key: &str, _evicted: &[SessionMessage]) {
        // No compression for stateless context
    }

    async fn recall_history(
        &self,
        _key: &str,
        _query_embedding: &[f32],
        _top_k: usize,
    ) -> anyhow::Result<Vec<String>> {
        // No history recall for stateless context
        Ok(Vec::new())
    }

    fn is_persistent(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_stateless_context_is_not_persistent() {
        let context = StatelessContext::new();
        assert!(!context.is_persistent());
    }

    #[tokio::test]
    async fn test_stateless_context_load_session() {
        let context = StatelessContext::new();
        let key = SessionKey::new(crate::bus::ChannelType::Cli, "test");
        let session = context.load_session(&key).await;
        // Session stores key as String, compare with the string representation
        assert_eq!(session.key, key.to_string());
    }

    #[tokio::test]
    async fn test_stateless_context_no_summary() {
        let context = StatelessContext::new();
        let summary = context.load_summary("test").await;
        assert!(summary.is_none());
    }
}
