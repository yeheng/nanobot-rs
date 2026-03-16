//! Session management
//!
//! Sessions are persisted in SQLite via `SqliteStore` with per-message storage.
//! Each message is stored as a separate row for O(1) append operations.
//! Legacy JSON blob sessions are automatically migrated on first load.
//!
//! **No in-memory cache**: SQLite is the single source of truth. This
//! eliminates the race condition where concurrent callers clone-modify-overwrite
//! the same session in a HashMap, silently losing messages.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument, warn};

use crate::bus::events::SessionKey;
use crate::memory::SqliteStore;
use crate::providers::MessageRole;

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
}

impl Session {
    /// Create a new session from a SessionKey.
    pub fn from_key(key: SessionKey) -> Self {
        Self {
            key: key.to_string(),
            messages: Vec::new(),
            last_consolidated: 0,
        }
    }

    /// Create a new session
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            messages: Vec::new(),
            last_consolidated: 0,
        }
    }

    /// Get the session key as a typed SessionKey.
    ///
    /// # Safety
    ///
    /// This is safe because `self.key` is always generated from a valid `SessionKey::to_string()`,
    /// which guarantees the format is correct ("channel:chat_id").
    pub fn session_key(&self) -> SessionKey {
        // SAFETY: self.key is always generated from SessionKey::to_string()
        SessionKey::from(self.key.as_str())
    }

    /// Add a message to the session (in-memory only; caller must persist separately)
    pub fn add_message(
        &mut self,
        role: MessageRole,
        content: &str,
        tools_used: Option<Vec<String>>,
    ) {
        self.messages.push(SessionMessage {
            role,
            content: content.to_string(),
            timestamp: Utc::now(),
            tools_used,
        });
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
    }
}

/// A message in a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub tools_used: Option<Vec<String>>,
}

/// Session manager backed by SQLite (no in-memory cache).
///
/// Every read goes directly to SQLite. This eliminates the clone-modify-overwrite
/// race condition that existed when an in-memory HashMap was used as a cache.
/// SQLite's page cache already provides efficient repeated reads.
pub struct SessionManager {
    store: SqliteStore,
}

impl SessionManager {
    /// Create a new session manager backed by the given `SqliteStore`.
    pub fn new(store: SqliteStore) -> Self {
        Self { store }
    }

    /// Get or create a session.
    ///
    /// Reads directly from SQLite; creates new if nothing exists.
    /// Automatically migrates legacy JSON blob sessions.
    #[instrument(name = "session.get_or_create", skip(self), fields(key = %key))]
    pub async fn get_or_create(&self, key: &SessionKey) -> Session {
        let key_str = key.to_string();
        // Try to load from SQLite (new per-message format)
        match self.load_session_from_db(&key_str).await {
            Ok(Some(s)) => {
                debug!("Loaded session {} from SQLite (per-message)", key_str);
                s
            }
            Ok(None) => {
                // Try legacy format migration
                match self.try_migrate_legacy_session(&key_str).await {
                    Ok(Some(s)) => {
                        debug!("Migrated session {} from legacy format", key_str);
                        s
                    }
                    _ => Session::from_key(key.clone()),
                }
            }
            Err(e) => {
                warn!("Failed to load session {}: {}, creating new", key_str, e);
                Session::from_key(key.clone())
            }
        }
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
                let role: MessageRole = row.role.parse().unwrap_or(MessageRole::User);
                SessionMessage {
                    role,
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
        };

        // Persist all messages from legacy format into per-message storage
        self.migrate_legacy_to_sqlite(&session).await?;

        Ok(Some(session))
    }

    /// Save a session after clear().
    ///
    /// When the session has been cleared (messages empty), only deletes
    /// existing messages and updates metadata — no pointless INSERT loop.
    /// For sessions with messages this is a full rewrite (safety fallback).
    #[instrument(name = "session.save", skip(self), fields(key = %session.key))]
    pub async fn save(&self, session: &Session) {
        let result = if session.messages.is_empty() {
            // Fast path for clear(): just delete + meta update
            self.clear_and_save_meta(&session.key, session.last_consolidated)
                .await
        } else {
            // Full rewrite (safety fallback for edge cases)
            self.migrate_legacy_to_sqlite(session).await
        };

        if let Err(e) = result {
            warn!("Failed to save session {} to SQLite: {}", session.key, e);
        }
    }

    /// Clear session messages in the DB and update metadata.
    ///
    /// Pure DELETE + meta upsert — O(1), no message re-insertion.
    async fn clear_and_save_meta(&self, key: &str, last_consolidated: usize) -> anyhow::Result<()> {
        self.store.clear_session_messages(key).await?;
        self.store.save_session_meta(key, last_consolidated).await?;
        debug!("Cleared session messages: {}", key);
        Ok(())
    }

    /// Migrate a legacy session by writing all its messages to per-message storage.
    ///
    /// This is the only path that does DELETE-all + INSERT-all, and it only
    /// runs during legacy JSON → SQLite migration.
    async fn migrate_legacy_to_sqlite(&self, session: &Session) -> anyhow::Result<()> {
        // Clear any existing messages (idempotent)
        self.store.clear_session_messages(&session.key).await?;

        // Save metadata
        self.store
            .save_session_meta(&session.key, session.last_consolidated)
            .await?;

        // Insert all messages from the legacy snapshot
        for msg in &session.messages {
            self.store
                .append_session_message(
                    &session.key,
                    msg.role.as_str(),
                    &msg.content,
                    &msg.timestamp,
                    msg.tools_used.as_deref(),
                )
                .await?;
        }

        debug!(
            "Migrated session to SQLite: {} ({} messages)",
            session.key,
            session.messages.len()
        );
        Ok(())
    }

    /// Clear a session's messages in both memory and database.
    ///
    /// Preferred method for `/new`-style reset: directly issues a DELETE
    /// against the DB without loading the full session first.
    /// Preserves the last_consolidated value to maintain summary context.
    #[instrument(name = "session.clear", skip(self), fields(key = %key))]
    pub async fn clear_session(&self, key: &SessionKey) {
        let key_str = key.to_string();

        // Load existing last_consolidated value to preserve summary context
        let last_consolidated = match self.store.load_session_meta(&key_str).await {
            Ok(Some(meta)) => meta.last_consolidated,
            _ => 0,
        };

        if let Err(e) = self.clear_and_save_meta(&key_str, last_consolidated).await {
            warn!("Failed to clear session {} in SQLite: {}", key_str, e);
        }
    }

    /// Append a single message to session (O(1) operation).
    ///
    /// Adds the message to the caller's in-memory `Session` *and* persists it
    /// to SQLite with a single INSERT. No global cache is updated, so
    /// concurrent callers operating on different snapshots cannot overwrite
    /// each other's messages in the database.
    #[instrument(name = "session.append_message", skip(self), fields(key = %session.key))]
    pub async fn append_message(
        &self,
        session: &mut Session,
        role: MessageRole,
        content: &str,
        tools_used: Option<Vec<String>>,
    ) -> anyhow::Result<()> {
        let timestamp = Utc::now();

        // Add to the caller's in-memory session so the current conversation
        // can continue building on it.
        session.messages.push(SessionMessage {
            role: role.clone(),
            content: content.to_string(),
            timestamp,
            tools_used: tools_used.clone(),
        });

        // Ensure session metadata exists (idempotent upsert)
        self.store
            .save_session_meta(&session.key, session.last_consolidated)
            .await?;

        // Persist to SQLite (single INSERT) — propagate errors
        self.store
            .append_session_message(
                &session.key,
                role.as_str(),
                content,
                &timestamp,
                tools_used.as_deref(),
            )
            .await?;

        Ok(())
    }

    /// Append a single message to a session by key (O(1), stateless).
    ///
    /// Persists directly to SQLite without requiring a pre-loaded `Session`
    /// object.  This is the preferred method for hooks that must remain
    /// stateless — no in-memory session cache is consulted or modified.
    #[instrument(name = "session.append_by_key", skip(self, content), fields(key = %session_key))]
    pub async fn append_by_key(
        &self,
        session_key: &SessionKey,
        role: &str,
        content: &str,
        tools_used: Option<Vec<String>>,
    ) -> anyhow::Result<()> {
        let key_str = session_key.to_string();
        let timestamp = Utc::now();

        // Ensure session metadata exists (idempotent upsert)
        self.store.save_session_meta(&key_str, 0).await?;

        // Persist to SQLite (single INSERT)
        self.store
            .append_session_message(&key_str, role, content, &timestamp, tools_used.as_deref())
            .await?;

        Ok(())
    }

    /// Invalidate a session (removes from SQLite).
    pub async fn invalidate(&self, key: &SessionKey) {
        let key_str = key.to_string();
        if let Err(e) = self.store.delete_session(&key_str).await {
            warn!("Failed to delete session {} from SQLite: {}", key_str, e);
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
