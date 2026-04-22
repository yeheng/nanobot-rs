//! Event store for event sourcing architecture.

use crate::processor::count_tokens;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gasket_types::{EventMetadata, EventType, SessionEvent, SessionKey, TokenUsage};
use serde::{Deserialize, Serialize};

use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tracing::{debug, info};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Invalid UUID: {0}")]
    InvalidUuid(String),

    #[error("Invalid event type: {0}")]
    InvalidEventType(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum EventData {
    ToolCall {
        tool_name: String,
        arguments: serde_json::Value,
    },
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        is_error: bool,
    },
    Summary {
        summary_type: String,
        summary_topic: Option<String>,
        covered_event_ids: Vec<String>,
    },
}

impl EventData {
    fn from_event_type(et: &EventType) -> Option<Self> {
        match et {
            EventType::ToolCall {
                tool_name,
                arguments,
            } => Some(EventData::ToolCall {
                tool_name: tool_name.clone(),
                arguments: arguments.clone(),
            }),
            EventType::ToolResult {
                tool_call_id,
                tool_name,
                is_error,
            } => Some(EventData::ToolResult {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                is_error: *is_error,
            }),
            EventType::Summary {
                summary_type,
                covered_event_ids,
            } => {
                let (stype, topic) = match summary_type {
                    gasket_types::SummaryType::TimeWindow { duration_hours } => {
                        (format!("time_window:{}", duration_hours), None)
                    }
                    gasket_types::SummaryType::Topic { topic } => {
                        ("topic".into(), Some(topic.clone()))
                    }
                    gasket_types::SummaryType::Compression { token_budget } => {
                        (format!("compression:{}", token_budget), None)
                    }
                };
                Some(EventData::Summary {
                    summary_type: stype,
                    summary_topic: topic,
                    covered_event_ids: covered_event_ids.iter().map(|id| id.to_string()).collect(),
                })
            }
            EventType::UserMessage | EventType::AssistantMessage => None,
        }
    }

    fn to_event_type(&self) -> Result<EventType, StoreError> {
        Ok(match self {
            EventData::ToolCall {
                tool_name,
                arguments,
            } => EventType::ToolCall {
                tool_name: tool_name.clone(),
                arguments: arguments.clone(),
            },
            EventData::ToolResult {
                tool_call_id,
                tool_name,
                is_error,
            } => EventType::ToolResult {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                is_error: *is_error,
            },
            EventData::Summary {
                summary_type,
                summary_topic,
                covered_event_ids,
            } => {
                let stype = match summary_type.as_str() {
                    s if s.starts_with("time_window:") => {
                        let hours: u32 = s.split(':').nth(1).unwrap_or("0").parse().unwrap_or(0);
                        gasket_types::SummaryType::TimeWindow {
                            duration_hours: hours,
                        }
                    }
                    "topic" => gasket_types::SummaryType::Topic {
                        topic: summary_topic.clone().unwrap_or_default(),
                    },
                    s if s.starts_with("compression:") => {
                        let budget: usize = s.split(':').nth(1).unwrap_or("0").parse().unwrap_or(0);
                        gasket_types::SummaryType::Compression {
                            token_budget: budget,
                        }
                    }
                    _ => gasket_types::SummaryType::Compression { token_budget: 0 },
                };
                let covered: Vec<Uuid> = covered_event_ids
                    .iter()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                EventType::Summary {
                    summary_type: stype,
                    covered_event_ids: covered,
                }
            }
        })
    }
}

fn event_type_tag(et: &EventType) -> &'static str {
    match et {
        EventType::UserMessage => "user_message",
        EventType::AssistantMessage => "assistant_message",
        EventType::ToolCall { .. } => "tool_call",
        EventType::ToolResult { .. } => "tool_result",
        EventType::Summary { .. } => "summary",
    }
}

/// Filter for querying events from the store.
#[derive(Debug, Default)]
pub struct EventFilter {
    pub session_key: Option<SessionKey>,
    pub time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
    pub event_types: Option<Vec<EventType>>,
    pub event_ids: Option<Vec<Uuid>>,
    pub limit: Option<usize>,

    /// For checkpoint-based recovery: only return events with sequence > this value.
    pub sequence_after: Option<i64>,
}

/// Event store trait — narrow interface for event log operations.
///
/// Implementors provide: append, query, and subscribe.
/// NOT included: truncation, summary management, embedding generation.
#[async_trait]
pub trait EventStoreTrait: Send + Sync {
    /// Append an event and return its assigned sequence number.
    async fn append(&self, event: &SessionEvent) -> Result<i64, StoreError>;

    /// Query events matching the given filter.
    async fn query_events(&self, filter: &EventFilter) -> Result<Vec<SessionEvent>, StoreError>;

    /// Subscribe to newly appended events via broadcast channel.
    fn subscribe(&self) -> broadcast::Receiver<SessionEvent>;

    /// Get the latest summary event for a session.
    async fn get_latest_summary(
        &self,
        session_key: &SessionKey,
    ) -> Result<Option<SessionEvent>, StoreError>;
}

pub struct EventStore {
    pool: SqlitePool,
    tx: broadcast::Sender<SessionEvent>,
}

impl EventStore {
    pub fn new(pool: SqlitePool) -> Self {
        let (tx, _) = broadcast::channel(64);
        info!("EventStore created");
        Self { pool, tx }
    }

    /// Parse a session key string into (channel, chat_id).
    /// Falls back to ChannelType::Cli if no channel prefix.
    fn parse_session_key_str(session_key: &str) -> (String, String) {
        let key = SessionKey::parse(session_key)
            .unwrap_or_else(|| SessionKey::new(gasket_types::ChannelType::Cli, session_key));
        (key.channel.to_string(), key.chat_id)
    }

    async fn generate_sequence(&self, session_key: &str) -> Result<i64, StoreError> {
        let (channel, chat_id) = Self::parse_session_key_str(session_key);
        let max_seq: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence), 0) FROM session_events WHERE channel = ? AND chat_id = ?",
        )
        .bind(&channel)
        .bind(&chat_id)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        Ok(max_seq + 1)
    }

    async fn append_event_with_sequence(
        &self,
        event: &SessionEvent,
        sequence: i64,
    ) -> Result<(), StoreError> {
        let event_type_tag = event_type_tag(&event.event_type);
        let event_data = EventData::from_event_type(&event.event_type);
        let event_data_json = event_data.as_ref().map(serde_json::to_string).transpose()?;
        let tools_used = serde_json::to_string(&event.metadata.tools_used)?;
        let token_usage = event
            .metadata
            .token_usage
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let extra = serde_json::to_string(&event.metadata.extra)?;

        // Parse session_key into channel/chat_id
        let (channel, chat_id) = Self::parse_session_key_str(&event.session_key);

        let mut tx = self.pool.begin().await?;

        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT OR IGNORE INTO sessions_v2 (key, channel, chat_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&event.session_key)
        .bind(&channel)
        .bind(&chat_id)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        // Use pre-computed token count if caller already set it, otherwise compute now.
        // This avoids redundant BPE encoding when the caller (e.g. ContextBuilder) has
        // already counted tokens for in-memory events before persisting.
        let token_len = if event.metadata.content_token_len > 0 {
            event.metadata.content_token_len as i64
        } else {
            count_tokens(&event.content) as i64
        };

        sqlx::query(
            r#"
            INSERT INTO session_events
            (id, session_key, channel, chat_id, event_type, content, embedding,
             tools_used, token_usage, token_len, event_data, extra, created_at, sequence)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(event.id.to_string())
        .bind(&event.session_key)
        .bind(&channel)
        .bind(&chat_id)
        .bind(event_type_tag)
        .bind(&event.content)
        // Embedding is no longer stored in the event row. Semantic embeddings
        // are written separately to `session_embeddings` by `AgentContext::save_event`.
        .bind(None::<&[u8]>)
        .bind(&tools_used)
        .bind(token_usage.as_deref())
        .bind(token_len)
        .bind(event_data_json.as_deref())
        .bind(&extra)
        .bind(event.created_at.to_rfc3339())
        .bind(sequence)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "UPDATE sessions_v2 SET updated_at = ?, total_events = total_events + 1 WHERE channel = ? AND chat_id = ?",
        )
        .bind(&now)
        .bind(&channel)
        .bind(&chat_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        // Notify subscribers (ignore send errors — no subscribers is normal)
        let _ = self.tx.send(event.clone());

        debug!(
            "Appended event: type={}, session={}, seq={}",
            event_type_tag, event.session_key, sequence
        );

        Ok(())
    }

    pub async fn append_event(&self, event: &SessionEvent) -> Result<(), StoreError> {
        let sequence = self.generate_sequence(&event.session_key).await?;
        self.append_event_with_sequence(event, sequence).await
    }

    pub async fn get_session_history(
        &self,
        session_key: &SessionKey,
    ) -> Result<Vec<SessionEvent>, StoreError> {
        let channel = session_key.channel.to_string();
        let chat_id = &session_key.chat_id;
        let rows = sqlx::query_as::<_, EventRow>(
            r#"
            SELECT * FROM session_events
            WHERE channel = ? AND chat_id = ?
            ORDER BY created_at ASC
            "#,
        )
        .bind(&channel)
        .bind(chat_id)
        .fetch_all(&self.pool)
        .await?;

        debug!("Loaded {} events for session {}", rows.len(), session_key);
        rows.into_iter().map(|r| r.try_into()).collect()
    }

    /// Load events with sequence > after_sequence for a session (watermark-based query).
    ///
    /// This is the core read-path method for the watermark-based compaction design.
    /// Returns only events not yet covered by the summary's high-water mark,
    /// using the composite index on (session_key, sequence) for efficient lookup.
    pub async fn get_events_after_sequence(
        &self,
        session_key: &SessionKey,
        after_sequence: i64,
    ) -> Result<Vec<SessionEvent>, StoreError> {
        let channel = session_key.channel.to_string();
        let chat_id = &session_key.chat_id;
        let rows = sqlx::query_as::<_, EventRow>(
            r#"
            SELECT * FROM session_events
            WHERE channel = ? AND chat_id = ? AND sequence > ?
            ORDER BY sequence ASC
            "#,
        )
        .bind(&channel)
        .bind(chat_id)
        .bind(after_sequence)
        .fetch_all(&self.pool)
        .await?;

        debug!(
            "Loaded {} events after seq {} for {}",
            rows.len(),
            after_sequence,
            session_key
        );
        rows.into_iter().map(|r| r.try_into()).collect()
    }

    /// Load events with sequence <= target_sequence for compaction input.
    ///
    /// Returns events that are about to be summarized, excluding summary events
    /// to avoid circular references. Used by the compactor to gather input
    /// for LLM summarization.
    pub async fn get_events_up_to_sequence(
        &self,
        session_key: &SessionKey,
        target_sequence: i64,
    ) -> Result<Vec<SessionEvent>, StoreError> {
        let channel = session_key.channel.to_string();
        let chat_id = &session_key.chat_id;
        let rows = sqlx::query_as::<_, EventRow>(
            r#"
            SELECT * FROM session_events
            WHERE channel = ? AND chat_id = ? AND sequence <= ? AND event_type != 'summary'
            ORDER BY sequence ASC
            "#,
        )
        .bind(&channel)
        .bind(chat_id)
        .bind(target_sequence)
        .fetch_all(&self.pool)
        .await?;

        debug!(
            "Loaded {} events up to seq {} for {} (excl. summaries)",
            rows.len(),
            target_sequence,
            session_key
        );
        rows.into_iter().map(|r| r.try_into()).collect()
    }

    pub async fn get_events_by_ids(
        &self,
        session_key: &SessionKey,
        event_ids: &[Uuid],
    ) -> Result<Vec<SessionEvent>, StoreError> {
        if event_ids.is_empty() {
            return Ok(vec![]);
        }

        let channel = session_key.channel.to_string();
        let chat_id = &session_key.chat_id;
        let placeholders: String = event_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let query = format!(
            "SELECT * FROM session_events WHERE channel = ? AND chat_id = ? AND id IN ({}) ORDER BY created_at ASC",
            placeholders
        );

        let mut sql_query = sqlx::query_as::<_, EventRow>(&query);
        sql_query = sql_query.bind(&channel).bind(chat_id);
        for id in event_ids {
            sql_query = sql_query.bind(id.to_string());
        }

        let rows = sql_query.fetch_all(&self.pool).await?;
        rows.into_iter().map(|r| r.try_into()).collect()
    }

    pub async fn clear_session(&self, session_key: &SessionKey) -> Result<(), StoreError> {
        let channel = session_key.channel.to_string();
        let chat_id = &session_key.chat_id;
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM session_events WHERE channel = ? AND chat_id = ?")
            .bind(&channel)
            .bind(chat_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM sessions_v2 WHERE channel = ? AND chat_id = ?")
            .bind(&channel)
            .bind(chat_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        info!("Cleared session {}", session_key);
        Ok(())
    }

    /// Get the maximum sequence number for a session.
    ///
    /// Returns 0 if the session has no events. Used by the compaction
    /// pipeline to determine the current high-water mark.
    pub async fn get_max_sequence(&self, session_key: &SessionKey) -> Result<i64, StoreError> {
        let max_seq: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence), 0) FROM session_events WHERE channel = ? AND chat_id = ?",
        )
        .bind(session_key.channel.to_string())
        .bind(&session_key.chat_id)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        Ok(max_seq)
    }

    /// Garbage-collect events that have been summarized.
    ///
    /// Deletes all events with `sequence <= target_sequence` for the given session.
    /// This is called after a successful compaction — the summary's
    /// `covered_upto_sequence` watermark guarantees these events are covered.
    pub async fn delete_events_upto(
        &self,
        session_key: &SessionKey,
        target_sequence: i64,
    ) -> Result<u64, StoreError> {
        let channel = session_key.channel.to_string();
        let chat_id = &session_key.chat_id;
        let result = sqlx::query(
            "DELETE FROM session_events WHERE channel = ? AND chat_id = ? AND sequence <= ?",
        )
        .bind(&channel)
        .bind(chat_id)
        .bind(target_sequence)
        .execute(&self.pool)
        .await?;
        let deleted = result.rows_affected();
        info!(
            "GC'd {} events up to seq {} for {}",
            deleted, target_sequence, session_key
        );
        Ok(deleted)
    }

    /// Get the most recent summary event for a session.
    ///
    /// Returns the latest `EventType::Summary` event, which serves as a
    /// checkpoint for context reconstruction. Used by the compression
    /// pipeline to load the existing summary before generating a new one.
    pub async fn get_latest_summary(
        &self,
        session_key: &SessionKey,
    ) -> Result<Option<SessionEvent>, StoreError> {
        let session_key_str = session_key.to_string();
        let row = sqlx::query_as::<_, EventRow>(
            r#"
            SELECT * FROM session_events
            WHERE session_key = ? AND event_type = 'summary'
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(&session_key_str)
        .fetch_optional(&self.pool)
        .await?;

        let found = row.is_some();
        let result = row.map(|r| r.try_into()).transpose();
        debug!("Latest summary for {}: found={}", session_key, found);
        result
    }

    /// Query all sessions for a given channel.
    ///
    /// Returns session keys ordered by updated_at descending.
    pub async fn get_sessions_by_channel(&self, channel: &str) -> Result<Vec<String>, StoreError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT key FROM sessions_v2 WHERE channel = ? ORDER BY updated_at DESC",
        )
        .bind(channel)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(k,)| k).collect())
    }

    /// Query all sessions for a given chat_id across channels.
    ///
    /// Returns session keys ordered by updated_at descending.
    pub async fn get_sessions_by_chat_id(&self, chat_id: &str) -> Result<Vec<String>, StoreError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT key FROM sessions_v2 WHERE chat_id = ? ORDER BY updated_at DESC",
        )
        .bind(chat_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(k,)| k).collect())
    }
}

#[async_trait]
impl EventStoreTrait for EventStore {
    async fn append(&self, event: &SessionEvent) -> Result<i64, StoreError> {
        let sequence = self.generate_sequence(&event.session_key).await?;
        self.append_event_with_sequence(event, sequence).await?;
        Ok(sequence)
    }

    async fn query_events(&self, filter: &EventFilter) -> Result<Vec<SessionEvent>, StoreError> {
        let session_key = match &filter.session_key {
            Some(k) => k.clone(),
            None => return Ok(vec![]),
        };
        let mut events = self.get_session_history(&session_key).await?;

        // Apply filters
        if let Some(time_range) = &filter.time_range {
            events.retain(|e| e.created_at >= time_range.0 && e.created_at <= time_range.1);
        }
        if let Some(event_types) = &filter.event_types {
            events.retain(|e| {
                event_types.iter().any(|et| {
                    // Match event types by variant kind, ignoring data fields
                    matches!(
                        (&e.event_type, et),
                        (EventType::UserMessage, EventType::UserMessage)
                            | (EventType::AssistantMessage, EventType::AssistantMessage)
                            | (EventType::ToolCall { .. }, EventType::ToolCall { .. })
                            | (EventType::ToolResult { .. }, EventType::ToolResult { .. })
                            | (EventType::Summary { .. }, EventType::Summary { .. })
                    )
                })
            });
        }
        if let Some(sequence_after) = filter.sequence_after {
            events.retain(|e| e.sequence > sequence_after);
        }
        if let Some(event_ids) = &filter.event_ids {
            let id_set: std::collections::HashSet<Uuid> = event_ids.iter().copied().collect();
            events.retain(|e| id_set.contains(&e.id));
        }
        if let Some(limit) = filter.limit {
            events.truncate(limit);
        }
        debug!("Query returned {} events for {}", events.len(), session_key);
        Ok(events)
    }

    fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.tx.subscribe()
    }

    async fn get_latest_summary(
        &self,
        session_key: &SessionKey,
    ) -> Result<Option<SessionEvent>, StoreError> {
        self.get_latest_summary(session_key).await
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct EventRow {
    id: String,
    session_key: String,
    channel: String,
    chat_id: String,
    event_type: String,
    content: String,
    #[allow(dead_code)]
    embedding: Option<Vec<u8>>,
    tools_used: String,
    token_usage: Option<String>,
    token_len: i64,
    event_data: Option<String>,
    extra: String,
    created_at: String,
    sequence: i64,
}

impl TryFrom<EventRow> for SessionEvent {
    type Error = StoreError;

    fn try_from(row: EventRow) -> Result<Self, Self::Error> {
        let event_type = match row.event_type.as_str() {
            "user_message" => EventType::UserMessage,
            "assistant_message" => EventType::AssistantMessage,
            "tool_call" | "tool_result" | "summary" => {
                let data: EventData =
                    serde_json::from_str(row.event_data.as_deref().unwrap_or("{}"))?;
                data.to_event_type()?
            }
            _ => return Err(StoreError::InvalidEventType(row.event_type)),
        };

        let tools_used: Vec<String> = serde_json::from_str(&row.tools_used)?;
        let token_usage: Option<TokenUsage> = row
            .token_usage
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?;
        let extra: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&row.extra)?;

        // Reconstruct session_key from channel/chat_id for backward compatibility
        let session_key = if !row.channel.is_empty() && !row.chat_id.is_empty() {
            format!("{}:{}", row.channel, row.chat_id)
        } else {
            row.session_key
        };

        Ok(SessionEvent {
            id: row
                .id
                .parse()
                .map_err(|_| StoreError::InvalidUuid(row.id.clone()))?,
            session_key,
            event_type,
            content: row.content,
            metadata: EventMetadata {
                tools_used,
                token_usage,
                content_token_len: row.token_len as usize,
                extra,
            },
            created_at: DateTime::parse_from_rfc3339(&row.created_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            sequence: row.sequence,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use gasket_types::{EventMetadata, EventType, SummaryType};
    use sqlx::sqlite::SqlitePoolOptions;
    use uuid::Uuid;

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePoolOptions::new().connect(":memory:").await.unwrap();

        sqlx::query(
            r#"
            CREATE TABLE sessions_v2 (
                key TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                last_consolidated_event TEXT,
                total_events INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                channel TEXT NOT NULL DEFAULT '',
                chat_id TEXT NOT NULL DEFAULT ''
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE session_events (
                id TEXT PRIMARY KEY,
                session_key TEXT NOT NULL,
                channel TEXT NOT NULL DEFAULT '',
                chat_id TEXT NOT NULL DEFAULT '',
                event_type TEXT NOT NULL,
                content TEXT NOT NULL,
                embedding BLOB,
                tools_used TEXT DEFAULT '[]',
                token_usage TEXT,
                token_len INTEGER NOT NULL DEFAULT 0,
                event_data TEXT,
                extra TEXT DEFAULT '{}',
                created_at TEXT NOT NULL,
                sequence INTEGER NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_events_channel_chat ON session_events(channel, chat_id)",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_sessions_v2_channel_chat ON sessions_v2(channel, chat_id)",
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    #[tokio::test]
    async fn test_append_user_message() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        let event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::UserMessage,
            content: "Hello, world!".into(),
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
            sequence: 0,
        };

        store.append_event(&event).await.unwrap();

        let count: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM session_events")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(count.0, 1);
    }

    #[tokio::test]
    async fn test_append_tool_call() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        let event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::ToolCall {
                tool_name: "read_file".into(),
                arguments: serde_json::json!({"path": "/test.txt"}),
            },
            content: "".into(),
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
            sequence: 0,
        };

        store.append_event(&event).await.unwrap();

        let history = store
            .get_session_history(&SessionKey::parse("test:session").unwrap())
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        match &history[0].event_type {
            EventType::ToolCall {
                tool_name,
                arguments,
            } => {
                assert_eq!(tool_name, "read_file");
                assert_eq!(arguments, &serde_json::json!({"path": "/test.txt"}));
            }
            _ => panic!("Expected ToolCall"),
        }
    }

    #[tokio::test]
    async fn test_append_tool_result() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        let event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::ToolResult {
                tool_call_id: "call_123".into(),
                tool_name: "read_file".into(),
                is_error: false,
            },
            content: "file contents".into(),
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
            sequence: 0,
        };

        store.append_event(&event).await.unwrap();

        let history = store
            .get_session_history(&SessionKey::parse("test:session").unwrap())
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        match &history[0].event_type {
            EventType::ToolResult {
                tool_call_id,
                tool_name,
                is_error,
            } => {
                assert_eq!(tool_call_id, "call_123");
                assert_eq!(tool_name, "read_file");
                assert!(!is_error);
            }
            _ => panic!("Expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn test_append_summary_event() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        let covered_ids = vec![Uuid::now_v7(), Uuid::now_v7()];
        let event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::Summary {
                summary_type: SummaryType::Topic {
                    topic: "discussion about API".into(),
                },
                covered_event_ids: covered_ids.clone(),
            },
            content: "Summary of the discussion...".into(),
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
            sequence: 0,
        };

        store.append_event(&event).await.unwrap();

        let history = store
            .get_session_history(&SessionKey::parse("test:session").unwrap())
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        match &history[0].event_type {
            EventType::Summary {
                summary_type,
                covered_event_ids,
            } => {
                match summary_type {
                    SummaryType::Topic { topic } => assert_eq!(topic, "discussion about API"),
                    _ => panic!("Expected Topic"),
                }
                assert_eq!(covered_event_ids, &covered_ids);
            }
            _ => panic!("Expected Summary"),
        }
    }

    #[tokio::test]
    async fn test_session_auto_created() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        let event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "auto:session".into(),
            event_type: EventType::UserMessage,
            content: "Test".into(),
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
            sequence: 0,
        };

        store.append_event(&event).await.unwrap();

        let count: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM sessions_v2")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(count.0, 1);

        let total_events: (i32,) =
            sqlx::query_as("SELECT total_events FROM sessions_v2 WHERE key = 'auto:session'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(total_events.0, 1);
    }

    #[tokio::test]
    async fn test_append_event_embedding_column_is_null() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        let event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::UserMessage,
            content: "Message without embedding field".into(),
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
            sequence: 0,
        };

        store.append_event(&event).await.unwrap();

        let row: (Option<Vec<u8>>,) = sqlx::query_as(
            "SELECT embedding FROM session_events WHERE session_key = 'test:session'",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();

        // Embedding is no longer stored in the event row; it is written
        // separately to `session_embeddings` by `AgentContext::save_event`.
        assert!(row.0.is_none(), "embedding column should be NULL");
    }

    #[tokio::test]
    async fn test_get_session_history() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        let e1 = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::UserMessage,
            content: "Hello".into(),
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
            sequence: 0,
        };
        store.append_event(&e1).await.unwrap();

        let e2 = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::AssistantMessage,
            content: "Hi!".into(),
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
            sequence: 0,
        };
        store.append_event(&e2).await.unwrap();

        let history = store
            .get_session_history(&SessionKey::parse("test:session").unwrap())
            .await
            .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content, "Hello");
        assert_eq!(history[1].content, "Hi!");
    }

    #[tokio::test]
    async fn test_get_session_history_empty() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        let history = store
            .get_session_history(&SessionKey::parse("nonexistent:session").unwrap())
            .await
            .unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn test_get_events_by_ids() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        let e1 = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::UserMessage,
            content: "Event 1".into(),
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
            sequence: 0,
        };
        store.append_event(&e1).await.unwrap();

        let e2 = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::AssistantMessage,
            content: "Event 2".into(),
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
            sequence: 0,
        };
        store.append_event(&e2).await.unwrap();

        let e3 = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::UserMessage,
            content: "Event 3".into(),
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
            sequence: 0,
        };
        store.append_event(&e3).await.unwrap();

        let key = SessionKey::parse("test:session").unwrap();
        let events = store
            .get_events_by_ids(&key, &[e1.id, e3.id])
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].content, "Event 1");
        assert_eq!(events[1].content, "Event 3");

        let events = store
            .get_events_by_ids(&key, &[Uuid::now_v7()])
            .await
            .unwrap();
        assert!(events.is_empty());

        let events = store.get_events_by_ids(&key, &[]).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn test_clear_session() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        let e1 = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::UserMessage,
            content: "Event 1".into(),
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
            sequence: 0,
        };
        store.append_event(&e1).await.unwrap();

        let e2 = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::AssistantMessage,
            content: "Event 2".into(),
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
            sequence: 0,
        };
        store.append_event(&e2).await.unwrap();

        let count: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM session_events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count.0, 2);

        let session_count: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM sessions_v2")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(session_count.0, 1);

        store
            .clear_session(&SessionKey::parse("test:session").unwrap())
            .await
            .unwrap();

        let count: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM session_events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count.0, 0);

        let session_count: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM sessions_v2")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(session_count.0, 0);
    }
}
