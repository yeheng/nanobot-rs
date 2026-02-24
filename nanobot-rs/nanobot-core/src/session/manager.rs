//! Session management
//!
//! Sessions are persisted in SQLite via `SqliteStore` with per-message storage.
//! Each message is stored as a separate row for O(1) append operations.
//! Legacy JSON blob sessions are automatically migrated on first load.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, instrument, warn};

use crate::memory::SqliteStore;

/// A conversation session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Session key (e.g., "telegram:123456")
    pub key: String,

    /// Messages in the session
    pub messages: Vec<SessionMessage>,

    /// Last consolidation point
    #[serde(default)]
    pub last_consolidated: usize,

    /// Dirty flag - true if there are unsaved message additions
    #[serde(skip)]
    dirty: bool,
}

impl Session {
    /// Create a new session
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            messages: Vec::new(),
            last_consolidated: 0,
            dirty: false,
        }
    }

    /// Add a message to the session
    pub fn add_message(&mut self, role: &str, content: &str, tools_used: Option<Vec<String>>) {
        self.messages.push(SessionMessage {
            role: role.to_string(),
            content: content.to_string(),
            timestamp: Utc::now(),
            tools_used,
        });
        self.dirty = true;
    }

    /// Get the history as LLM messages (last N messages)
    pub fn get_history(&self, max_messages: usize) -> Vec<SessionMessage> {
        let start = self.messages.len().saturating_sub(max_messages);
        self.messages[start..].to_vec()
    }

    /// Clear the session
    pub fn clear(&mut self) {
        self.messages.clear();
        self.last_consolidated = 0;
        self.dirty = true;
    }

    /// Check if session has unsaved changes
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    #[allow(dead_code)]
    /// Mark session as saved
    fn mark_clean(&mut self) {
        self.dirty = false;
    }
}

/// A message in a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub tools_used: Option<Vec<String>>,
}

/// Session manager with in-memory cache and SQLite persistence.
///
/// Sessions are kept in an in-memory HashMap. Messages are appended individually
/// to SQLite for O(1) operations instead of rewriting the entire session.
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    store: SqliteStore,
}

impl SessionManager {
    /// Create a new session manager backed by the given `SqliteStore`.
    pub fn new(store: SqliteStore) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            store,
        }
    }

    /// Get or create a session.
    ///
    /// Reads from memory cache first; falls back to SQLite; creates new if
    /// neither exists. Automatically migrates legacy JSON blob sessions.
    #[instrument(name = "session.get_or_create", skip(self), fields(key = %key))]
    pub async fn get_or_create(&self, key: &str) -> Session {
        {
            let sessions = self.sessions.read().await;
            if let Some(session) = sessions.get(key) {
                return session.clone();
            }
        }

        let mut sessions = self.sessions.write().await;
        // Double check after acquiring write lock
        if let Some(session) = sessions.get(key) {
            return session.clone();
        }

        // Try to load from SQLite (new per-message format)
        let session = match self.load_session_from_db(key).await {
            Ok(Some(s)) => {
                debug!("Loaded session {} from SQLite (per-message)", key);
                s
            }
            Ok(None) => {
                // Try legacy format migration
                match self.try_migrate_legacy_session(key).await {
                    Ok(Some(s)) => {
                        debug!("Migrated session {} from legacy format", key);
                        s
                    }
                    _ => Session::new(key),
                }
            }
            Err(e) => {
                warn!("Failed to load session {}: {}, creating new", key, e);
                Session::new(key)
            }
        };

        sessions.insert(key.to_string(), session.clone());
        session
    }

    /// Load session from database using per-message storage.
    async fn load_session_from_db(&self, key: &str) -> anyhow::Result<Option<Session>> {
        // Load metadata
        let meta = match self.store.load_session_meta(key).await? {
            Some(m) => m,
            None => return Ok(None),
        };

        // Load messages
        let msg_rows = self.store.load_session_messages(key).await?;
        let messages: Vec<SessionMessage> = msg_rows
            .into_iter()
            .map(|row| {
                let tools_used = match row.tools_used {
                    Some(ref json) => serde_json::from_str(json).ok(),
                    None => None,
                };
                SessionMessage {
                    role: row.role,
                    content: row.content,
                    timestamp: row.timestamp,
                    tools_used,
                }
            })
            .collect();

        Ok(Some(Session {
            key: meta.key,
            messages,
            last_consolidated: meta.last_consolidated,
            dirty: false,
        }))
    }

    /// Try to migrate a legacy JSON blob session to per-message storage.
    async fn try_migrate_legacy_session(&self, key: &str) -> anyhow::Result<Option<Session>> {
        // Use deprecated method for backward compatibility
        #[allow(deprecated)]
        let data = match self.store.load_session(key).await? {
            Some(d) => d,
            None => return Ok(None),
        };

        // Parse legacy format
        let legacy_session: LegacySession = match serde_json::from_str(&data) {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to parse legacy session {}: {}", key, e);
                return Ok(None);
            }
        };

        // Migrate to new format
        let session = Session {
            key: legacy_session.key,
            messages: legacy_session.messages,
            last_consolidated: legacy_session.last_consolidated,
            dirty: false,
        };

        // Save in new format
        self.save_session_full(&session).await?;

        Ok(Some(session))
    }

    /// Save a session — appends new messages individually for O(1) operations.
    #[instrument(name = "session.save", skip(self), fields(key = %session.key))]
    pub async fn save(&self, session: &Session) {
        let key = session.key.clone();

        // Update in-memory cache
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(key.clone(), session.clone());
        }

        // Persist to SQLite
        if session.is_dirty() {
            // For dirty sessions, we need to check what's new
            // Since we don't track which messages are new, we do a full save
            if let Err(e) = self.save_session_full(session).await {
                warn!("Failed to save session {} to SQLite: {}", key, e);
            }
        } else {
            // Just update metadata
            if let Err(e) = self
                .store
                .save_session_meta(&key, session.last_consolidated)
                .await
            {
                warn!("Failed to save session meta {} to SQLite: {}", key, e);
            }
        }
    }

    /// Save session with all messages (used for new sessions or migrations).
    async fn save_session_full(&self, session: &Session) -> anyhow::Result<()> {
        // Clear existing messages
        self.store.clear_session_messages(&session.key).await?;

        // Save metadata
        self.store
            .save_session_meta(&session.key, session.last_consolidated)
            .await?;

        // Insert all messages
        for msg in &session.messages {
            self.store
                .append_session_message(
                    &session.key,
                    &msg.role,
                    &msg.content,
                    &msg.timestamp,
                    msg.tools_used.as_deref(),
                )
                .await?;
        }

        debug!(
            "Saved session full: {} ({} messages)",
            session.key,
            session.messages.len()
        );
        Ok(())
    }

    /// Append a single message to session (O(1) operation).
    /// This is the preferred way to add messages for better performance.
    ///
    /// Returns an error if the SQLite persist fails. The message is still
    /// added to the in-memory session so the current conversation can
    /// continue, but the caller should log / handle the error.
    #[instrument(name = "session.append_message", skip(self), fields(key = %session.key))]
    pub async fn append_message(
        &self,
        session: &mut Session,
        role: &str,
        content: &str,
        tools_used: Option<Vec<String>>,
    ) -> anyhow::Result<()> {
        let timestamp = Utc::now();

        // Add to in-memory session
        session.messages.push(SessionMessage {
            role: role.to_string(),
            content: content.to_string(),
            timestamp,
            tools_used: tools_used.clone(),
        });

        // Persist to SQLite (single INSERT) — propagate errors
        self.store
            .append_session_message(
                &session.key,
                role,
                content,
                &timestamp,
                tools_used.as_deref(),
            )
            .await?;

        // Update in-memory cache
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session.key.clone(), session.clone());
        }

        Ok(())
    }

    /// Invalidate a session from cache (also removes from SQLite).
    pub async fn invalidate(&self, key: &str) {
        let mut sessions = self.sessions.write().await;
        sessions.remove(key);

        if let Err(e) = self.store.delete_session(key).await {
            warn!("Failed to delete session {} from SQLite: {}", key, e);
        }
    }
}

/// Legacy session format for migration
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacySession {
    key: String,
    messages: Vec<SessionMessage>,
    #[serde(default)]
    last_consolidated: usize,
}
