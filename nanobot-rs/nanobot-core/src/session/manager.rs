//! Session management

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, instrument};

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

/// Cached session entry, tracks whether the session has unsaved changes.
struct CachedSession {
    session: Session,
    dirty: bool,
}

/// Session manager with in-memory cache and async disk persistence.
///
/// Sessions are kept in an LRU-style HashMap. Disk writes happen immediately
/// on `save()` calls.
///
/// All disk I/O uses `tokio::fs` to avoid blocking the async runtime.
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, CachedSession>>>,
    sessions_dir: PathBuf,
}

impl SessionManager {
    /// Create a new session manager
    pub async fn new(workspace: PathBuf) -> Self {
        let sessions_dir = workspace.join("sessions");
        let _ = tokio::fs::create_dir_all(&sessions_dir).await;

        let sessions = Arc::new(RwLock::new(HashMap::new()));

        Self {
            sessions,
            sessions_dir,
        }
    }

    /// Create a new session manager synchronously (for backwards compatibility)
    ///
    /// Note: This uses blocking I/O for directory creation. Prefer `new()` when possible.
    pub fn new_sync(workspace: PathBuf) -> Self {
        let sessions_dir = workspace.join("sessions");
        let _ = std::fs::create_dir_all(&sessions_dir);

        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            sessions_dir,
        }
    }

    /// Get or create a session.
    ///
    /// Reads from memory cache first; falls back to disk; creates new if
    /// neither exists. Does NOT write to disk.
    #[instrument(name = "session.get_or_create", skip(self), fields(key = %key))]
    pub async fn get_or_create(&self, key: &str) -> Session {
        {
            let sessions = self.sessions.read().await;
            if let Some(cached) = sessions.get(key) {
                return cached.session.clone();
            }
        }

        let mut sessions = self.sessions.write().await;
        // Double check after acquiring write lock
        if let Some(cached) = sessions.get(key) {
            return cached.session.clone();
        }

        // Try to load from disk
        let session = self
            .load_from_disk(key)
            .await
            .unwrap_or_else(|_| Session::new(key));
        sessions.insert(
            key.to_string(),
            CachedSession {
                session: session.clone(),
                dirty: false,
            },
        );
        session
    }

    /// Save a session — updates the in-memory cache and flushes immediately to disk.
    #[instrument(name = "session.save", skip(self), fields(key = %session.key))]
    pub async fn save(&self, session: &Session) {
        let key = session.key.clone();
        let mut sessions = self.sessions.write().await;
        sessions.insert(
            key.clone(),
            CachedSession {
                session: session.clone(),
                dirty: false,
            },
        );
        drop(sessions); // Release lock before I/O

        // Flush immediately - KISS
        let _ = Self::save_to_disk_internal(session, &self.sessions_dir).await;
    }

    /// Invalidate a session from cache
    pub async fn invalidate(&self, key: &str) {
        let mut sessions = self.sessions.write().await;
        sessions.remove(key);
    }

    fn session_path(&self, key: &str) -> PathBuf {
        // Sanitize key for filename
        let safe_key = key.replace(['/', ':', ' '], "_");
        self.sessions_dir.join(format!("{}.json", safe_key))
    }

    async fn load_from_disk(&self, key: &str) -> anyhow::Result<Session> {
        let path = self.session_path(key);
        let content = tokio::fs::read_to_string(&path).await.map_err(|e| {
            anyhow::anyhow!("Failed to read session file '{}': {}", path.display(), e)
        })?;
        let session: Session = serde_json::from_str(&content).map_err(|e| {
            anyhow::anyhow!("Failed to parse session file '{}': {}", path.display(), e)
        })?;
        debug!("Loaded session {} from disk", key);
        Ok(session)
    }

    async fn save_to_disk_internal(
        session: &Session,
        sessions_dir: &PathBuf,
    ) -> anyhow::Result<()> {
        let safe_key = session.key.replace(['/', ':', ' '], "_");
        let path = sessions_dir.join(format!("{}.json", safe_key));
        let tmp_path = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));

        // Serialize
        let content = serde_json::to_string_pretty(session)
            .map_err(|e| anyhow::anyhow!("Failed to serialize session '{}': {}", session.key, e))?;

        // Write to tmp using async file operations
        {
            use tokio::io::AsyncWriteExt;
            let mut file = tokio::fs::File::create(&tmp_path).await.map_err(|e| {
                anyhow::anyhow!(
                    "Failed to create tmp session file '{}': {}",
                    tmp_path.display(),
                    e
                )
            })?;
            file.write_all(content.as_bytes()).await.map_err(|e| {
                anyhow::anyhow!(
                    "Failed to write tmp session file '{}': {}",
                    tmp_path.display(),
                    e
                )
            })?;
            file.sync_all().await.map_err(|e| {
                anyhow::anyhow!(
                    "Failed to sync tmp session file '{}': {}",
                    tmp_path.display(),
                    e
                )
            })?;
        }

        // Rename atomically
        tokio::fs::rename(&tmp_path, &path).await.map_err(|e| {
            anyhow::anyhow!(
                "Failed to rename tmp session file '{}' to '{}': {}",
                tmp_path.display(),
                path.display(),
                e
            )
        })?;

        debug!("Saved session {} to disk", session.key);
        Ok(())
    }
}
