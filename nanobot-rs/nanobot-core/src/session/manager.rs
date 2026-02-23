//! Session management
//!
//! Sessions are persisted in SQLite via `SqliteStore` with an in-memory
//! cache for fast reads. Disk writes happen immediately on `save()` calls.

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
}

impl Session {
    /// Create a new session
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            messages: Vec::new(),
            last_consolidated: 0,
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
    pub role: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub tools_used: Option<Vec<String>>,
}

/// Session manager with in-memory cache and SQLite persistence.
///
/// Sessions are kept in an in-memory HashMap. SQLite writes happen immediately
/// on `save()` calls.
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
    /// neither exists. Does NOT write to SQLite.
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

        // Try to load from SQLite
        let session = match self.store.load_session(key).await {
            Ok(Some(data)) => match serde_json::from_str::<Session>(&data) {
                Ok(s) => {
                    debug!("Loaded session {} from SQLite", key);
                    s
                }
                Err(e) => {
                    warn!("Failed to parse session {}: {}, creating new", key, e);
                    Session::new(key)
                }
            },
            Ok(None) => Session::new(key),
            Err(e) => {
                warn!("Failed to load session {}: {}, creating new", key, e);
                Session::new(key)
            }
        };

        sessions.insert(key.to_string(), session.clone());
        session
    }

    /// Save a session — updates the in-memory cache and flushes immediately to SQLite.
    #[instrument(name = "session.save", skip(self), fields(key = %session.key))]
    pub async fn save(&self, session: &Session) {
        let key = session.key.clone();
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(key.clone(), session.clone());
        }

        // Flush to SQLite
        match serde_json::to_string(session) {
            Ok(data) => {
                if let Err(e) = self.store.save_session(&key, &data).await {
                    warn!("Failed to save session {} to SQLite: {}", key, e);
                }
            }
            Err(e) => {
                warn!("Failed to serialize session {}: {}", key, e);
            }
        }
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
