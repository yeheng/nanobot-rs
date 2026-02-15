//! Session management

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::debug;

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

/// Session manager
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    sessions_dir: PathBuf,
}

impl SessionManager {
    /// Create a new session manager
    pub fn new(workspace: PathBuf) -> Self {
        let sessions_dir = workspace.join("sessions");
        let _ = std::fs::create_dir_all(&sessions_dir);

        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            sessions_dir,
        }
    }

    /// Get or create a session
    pub async fn get_or_create(&self, key: &str) -> Session {
        let mut sessions = self.sessions.write().await;

        if let Some(session) = sessions.get(key) {
            return session.clone();
        }

        // Try to load from disk
        let session = self
            .load_from_disk(key)
            .unwrap_or_else(|_| Session::new(key));
        sessions.insert(key.to_string(), session.clone());
        session
    }

    /// Save a session
    pub async fn save(&self, session: &Session) {
        let mut sessions = self.sessions.write().await;
        sessions.insert(session.key.clone(), session.clone());

        // Persist to disk
        let _ = self.save_to_disk(session);
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

    fn load_from_disk(&self, key: &str) -> anyhow::Result<Session> {
        let path = self.session_path(key);
        let content = std::fs::read_to_string(path)?;
        let session: Session = serde_json::from_str(&content)?;
        debug!("Loaded session {} from disk", key);
        Ok(session)
    }

    fn save_to_disk(&self, session: &Session) -> anyhow::Result<()> {
        let path = self.session_path(&session.key);
        let content = serde_json::to_string_pretty(session)?;
        std::fs::write(path, content)?;
        debug!("Saved session {} to disk", session.key);
        Ok(())
    }
}
