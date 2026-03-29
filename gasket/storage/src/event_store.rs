//! Event store for event sourcing architecture.

use chrono::{DateTime, Utc};
use gasket_types::{EventMetadata, EventType, SessionEvent, SummaryType, TokenUsage};
use serde_json::Value;
use sqlx::SqlitePool;
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

/// Event store - core of event sourcing architecture.
pub struct EventStore {
    pool: SqlitePool,
}

impl EventStore {
    /// Create a new event store.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Append an event (O(1) operation).
    ///
    /// All database operations are wrapped in a transaction to ensure atomicity.
    /// If any operation fails, all changes are rolled back.
    pub async fn append_event(&self, event: &SessionEvent) -> Result<(), StoreError> {
        let event_type_str = event_type_to_string(&event.event_type);
        let tools_used = serde_json::to_string(&event.metadata.tools_used)?;
        let token_usage = event
            .metadata
            .token_usage
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let extra = serde_json::to_string(&event.metadata.extra)?;

        // Extract event type specific fields
        let fields = extract_event_fields(&event.event_type);

        // Start transaction for atomic operations
        let mut tx = self.pool.begin().await?;

        // Ensure session exists
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT OR IGNORE INTO sessions_v2 (key, created_at, updated_at) VALUES (?, ?, ?)",
        )
        .bind(&event.session_key)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        // Insert event
        sqlx::query(
            r#"
            INSERT INTO session_events
            (id, session_key, parent_id, event_type, content, embedding, branch,
             tools_used, token_usage, tool_name, tool_arguments, tool_call_id, is_error,
             summary_type, summary_topic, covered_events, merge_source, merge_head, extra, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(event.id.to_string())
        .bind(&event.session_key)
        .bind(event.parent_id.map(|id| id.to_string()))
        .bind(&event_type_str)
        .bind(&event.content)
        .bind(event.embedding.as_ref().map(|e| bytemuck::cast_slice(e) as &[u8]))
        .bind(event.metadata.branch.as_deref().unwrap_or("main"))
        .bind(&tools_used)
        .bind(token_usage.as_deref())
        .bind(fields.tool_name.as_deref())
        .bind(fields.tool_arguments.as_deref())
        .bind(fields.tool_call_id.as_deref())
        .bind(fields.is_error)
        .bind(fields.summary_type.as_deref())
        .bind(fields.summary_topic.as_deref())
        .bind(fields.covered_events.as_deref())
        .bind(fields.merge_source.as_deref())
        .bind(fields.merge_head.as_deref())
        .bind(&extra)
        .bind(event.created_at.to_rfc3339())
        .execute(&mut *tx)
        .await?;

        // Update session metadata - read current branches, merge, and update
        let branch_name = event.metadata.branch.as_deref().unwrap_or("main");
        let current_branches: Option<String> =
            sqlx::query_scalar("SELECT branches FROM sessions_v2 WHERE key = ?")
                .bind(&event.session_key)
                .fetch_one(&mut *tx)
                .await?;

        let mut branches: Value = current_branches
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::json!({}));

        if let Some(obj) = branches.as_object_mut() {
            obj.insert(branch_name.to_string(), Value::String(event.id.to_string()));
        }

        let branches_str = serde_json::to_string(&branches)?;

        sqlx::query(
            "UPDATE sessions_v2 SET updated_at = ?, total_events = total_events + 1, branches = ? WHERE key = ?",
        )
        .bind(&now)
        .bind(&branches_str)
        .bind(&event.session_key)
        .execute(&mut *tx)
        .await?;

        // Commit transaction
        tx.commit().await?;

        Ok(())
    }

    /// Get branch history - retrieve all events for a session/branch ordered by time.
    ///
    /// Returns events in chronological order (oldest first).
    pub async fn get_branch_history(
        &self,
        session_key: &str,
        branch: &str,
    ) -> Result<Vec<SessionEvent>, StoreError> {
        let rows = sqlx::query_as::<_, EventRow>(
            r#"
            SELECT * FROM session_events
            WHERE session_key = ? AND branch = ?
            ORDER BY created_at ASC
            "#,
        )
        .bind(session_key)
        .bind(branch)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(|r| r.try_into()).collect()
    }

    /// Get events by their IDs - retrieve specific events for summarization.
    ///
    /// Returns events in chronological order (oldest first).
    /// Returns an empty vector if no events are found for the given IDs.
    pub async fn get_events_by_ids(
        &self,
        session_key: &str,
        event_ids: &[Uuid],
    ) -> Result<Vec<SessionEvent>, StoreError> {
        if event_ids.is_empty() {
            return Ok(vec![]);
        }

        let placeholders: String = event_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");

        let query = format!(
            "SELECT * FROM session_events WHERE session_key = ? AND id IN ({}) ORDER BY created_at ASC",
            placeholders
        );

        let mut sql_query = sqlx::query_as::<_, EventRow>(&query);
        sql_query = sql_query.bind(session_key);
        for id in event_ids {
            sql_query = sql_query.bind(id.to_string());
        }

        let rows = sql_query.fetch_all(&self.pool).await?;
        rows.into_iter().map(|r| r.try_into()).collect()
    }

    /// Clear all events for a session from the database.
    ///
    /// This is a destructive operation - all events will be permanently deleted.
    pub async fn clear_session(&self, session_key: &str) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;

        // Delete all events for this session
        sqlx::query("DELETE FROM session_events WHERE session_key = ?")
            .bind(session_key)
            .execute(&mut *tx)
            .await?;

        // Delete the session record
        sqlx::query("DELETE FROM sessions_v2 WHERE key = ?")
            .bind(session_key)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }
}

/// Database row representation for session events.
#[derive(Debug, Clone, sqlx::FromRow)]
struct EventRow {
    id: String,
    session_key: String,
    parent_id: Option<String>,
    event_type: String,
    content: String,
    embedding: Option<Vec<u8>>,
    branch: String,
    tools_used: String,
    token_usage: Option<String>,
    tool_name: Option<String>,
    tool_arguments: Option<String>,
    tool_call_id: Option<String>,
    is_error: Option<i32>,
    summary_type: Option<String>,
    summary_topic: Option<String>,
    covered_events: Option<String>,
    merge_source: Option<String>,
    merge_head: Option<String>,
    extra: String,
    created_at: String,
}

impl TryFrom<EventRow> for SessionEvent {
    type Error = StoreError;

    fn try_from(row: EventRow) -> Result<Self, Self::Error> {
        let event_type = parse_event_type(
            &row.event_type,
            row.tool_name.as_deref(),
            row.tool_arguments.as_deref(),
            row.tool_call_id.as_deref(),
            row.is_error,
            row.summary_type.as_deref(),
            row.summary_topic.as_deref(),
            row.covered_events.as_deref(),
            row.merge_source.as_deref(),
            row.merge_head.as_deref(),
        )?;

        let tools_used: Vec<String> = serde_json::from_str(&row.tools_used)?;
        let token_usage: Option<TokenUsage> = row
            .token_usage
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?;
        let extra: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&row.extra)?;
        let embedding = row.embedding.map(|b| bytemuck::cast_slice(&b).to_vec());

        Ok(SessionEvent {
            id: row
                .id
                .parse()
                .map_err(|_| StoreError::InvalidUuid(row.id.clone()))?,
            session_key: row.session_key,
            parent_id: row
                .parent_id
                .map(|s| s.parse())
                .transpose()
                .map_err(|_| StoreError::InvalidUuid("parent_id".into()))?,
            event_type,
            content: row.content,
            embedding,
            metadata: EventMetadata {
                branch: if row.branch == "main" {
                    None
                } else {
                    Some(row.branch)
                },
                tools_used,
                token_usage,
                extra,
            },
            created_at: DateTime::parse_from_rfc3339(&row.created_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        })
    }
}

/// Parse event type from database row fields.
#[allow(clippy::too_many_arguments)]
fn parse_event_type(
    type_str: &str,
    tool_name: Option<&str>,
    tool_arguments: Option<&str>,
    tool_call_id: Option<&str>,
    is_error: Option<i32>,
    summary_type: Option<&str>,
    summary_topic: Option<&str>,
    covered_events: Option<&str>,
    merge_source: Option<&str>,
    merge_head: Option<&str>,
) -> Result<EventType, StoreError> {
    Ok(match type_str {
        "user_message" => EventType::UserMessage,
        "assistant_message" => EventType::AssistantMessage,
        "tool_call" => EventType::ToolCall {
            tool_name: tool_name.unwrap_or("").into(),
            arguments: tool_arguments
                .map(|s| serde_json::from_str(s).unwrap_or(serde_json::Value::Null))
                .unwrap_or(serde_json::Value::Null),
        },
        "tool_result" => EventType::ToolResult {
            tool_call_id: tool_call_id.unwrap_or("").into(),
            tool_name: tool_name.unwrap_or("").into(),
            is_error: is_error.unwrap_or(0) != 0,
        },
        "summary" => {
            let covered: Vec<Uuid> = covered_events
                .map(serde_json::from_str::<Vec<String>>)
                .transpose()?
                .unwrap_or_default()
                .into_iter()
                .filter_map(|s| s.parse().ok())
                .collect();

            let stype = match summary_type {
                Some(s) if s.starts_with("time_window:") => {
                    let hours: u32 = s.split(':').nth(1).unwrap_or("0").parse().unwrap_or(0);
                    SummaryType::TimeWindow {
                        duration_hours: hours,
                    }
                }
                Some("topic") => SummaryType::Topic {
                    topic: summary_topic.unwrap_or("").into(),
                },
                Some(s) if s.starts_with("compression:") => {
                    let budget: usize = s.split(':').nth(1).unwrap_or("0").parse().unwrap_or(0);
                    SummaryType::Compression {
                        token_budget: budget,
                    }
                }
                _ => SummaryType::Compression { token_budget: 0 },
            };

            EventType::Summary {
                summary_type: stype,
                covered_event_ids: covered,
            }
        }
        "merge" => EventType::Merge {
            source_branch: merge_source.unwrap_or("").into(),
            source_head: merge_head
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(Uuid::nil),
        },
        _ => return Err(StoreError::InvalidEventType(type_str.into())),
    })
}

fn event_type_to_string(event_type: &EventType) -> String {
    match event_type {
        EventType::UserMessage => "user_message".into(),
        EventType::AssistantMessage => "assistant_message".into(),
        EventType::ToolCall { .. } => "tool_call".into(),
        EventType::ToolResult { .. } => "tool_result".into(),
        EventType::Summary { .. } => "summary".into(),
        EventType::Merge { .. } => "merge".into(),
    }
}

/// Extracted event-specific fields for database storage.
#[derive(Default)]
struct EventFields {
    tool_name: Option<String>,
    tool_arguments: Option<String>,
    tool_call_id: Option<String>,
    is_error: Option<i32>,
    summary_type: Option<String>,
    summary_topic: Option<String>,
    covered_events: Option<String>,
    merge_source: Option<String>,
    merge_head: Option<String>,
}

fn extract_event_fields(event_type: &EventType) -> EventFields {
    match event_type {
        EventType::ToolCall {
            tool_name,
            arguments,
        } => EventFields {
            tool_name: Some(tool_name.clone()),
            tool_arguments: Some(arguments.to_string()),
            ..Default::default()
        },
        EventType::ToolResult {
            tool_call_id,
            tool_name,
            is_error,
        } => EventFields {
            tool_name: Some(tool_name.clone()),
            tool_call_id: Some(tool_call_id.clone()),
            is_error: Some(*is_error as i32),
            ..Default::default()
        },
        EventType::Summary {
            summary_type,
            covered_event_ids,
        } => {
            let (stype, topic) = match summary_type {
                gasket_types::SummaryType::TimeWindow { duration_hours } => {
                    (Some(format!("time_window:{}", duration_hours)), None)
                }
                gasket_types::SummaryType::Topic { topic } => {
                    (Some("topic".into()), Some(topic.clone()))
                }
                gasket_types::SummaryType::Compression { token_budget } => {
                    (Some(format!("compression:{}", token_budget)), None)
                }
            };
            let covered = serde_json::to_string(
                &covered_event_ids
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>(),
            )
            .ok();
            EventFields {
                summary_type: stype,
                summary_topic: topic,
                covered_events: covered,
                ..Default::default()
            }
        }
        EventType::Merge {
            source_branch,
            source_head,
        } => EventFields {
            merge_source: Some(source_branch.clone()),
            merge_head: Some(source_head.to_string()),
            ..Default::default()
        },
        _ => EventFields::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use gasket_types::{EventMetadata, EventType};
    use sqlx::sqlite::SqlitePoolOptions;
    use uuid::Uuid;

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePoolOptions::new().connect(":memory:").await.unwrap();

        // Create tables
        sqlx::query(
            r#"
            CREATE TABLE sessions_v2 (
                key TEXT PRIMARY KEY,
                current_branch TEXT NOT NULL DEFAULT 'main',
                branches TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                last_consolidated_event TEXT,
                total_events INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0
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
                parent_id TEXT,
                event_type TEXT NOT NULL,
                content TEXT NOT NULL,
                embedding BLOB,
                branch TEXT DEFAULT 'main',
                tools_used TEXT DEFAULT '[]',
                token_usage TEXT,
                tool_name TEXT,
                tool_arguments TEXT,
                tool_call_id TEXT,
                is_error INTEGER DEFAULT 0,
                summary_type TEXT,
                summary_topic TEXT,
                covered_events TEXT,
                merge_source TEXT,
                merge_head TEXT,
                extra TEXT DEFAULT '{}',
                created_at TEXT NOT NULL
            )
            "#,
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
            parent_id: None,
            event_type: EventType::UserMessage,
            content: "Hello, world!".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };

        store.append_event(&event).await.unwrap();

        // Verify event was stored
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
            parent_id: None,
            event_type: EventType::ToolCall {
                tool_name: "read_file".into(),
                arguments: serde_json::json!({"path": "/test.txt"}),
            },
            content: "".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };

        store.append_event(&event).await.unwrap();

        // Verify tool_name was stored
        let row: (String,) =
            sqlx::query_as("SELECT tool_name FROM session_events WHERE event_type = 'tool_call'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(row.0, "read_file");
    }

    #[tokio::test]
    async fn test_append_tool_result() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        let event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            parent_id: None,
            event_type: EventType::ToolResult {
                tool_call_id: "call_123".into(),
                tool_name: "read_file".into(),
                is_error: false,
            },
            content: "file contents".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };

        store.append_event(&event).await.unwrap();

        // Verify tool_result fields were stored
        let row: (String, String, i32) = sqlx::query_as(
            "SELECT tool_name, tool_call_id, is_error FROM session_events WHERE event_type = 'tool_result'",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert_eq!(row.0, "read_file");
        assert_eq!(row.1, "call_123");
        assert_eq!(row.2, 0);
    }

    #[tokio::test]
    async fn test_append_summary_event() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        let covered_ids = vec![Uuid::now_v7(), Uuid::now_v7()];
        let event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            parent_id: None,
            event_type: EventType::Summary {
                summary_type: gasket_types::SummaryType::Topic {
                    topic: "discussion about API".into(),
                },
                covered_event_ids: covered_ids.clone(),
            },
            content: "Summary of the discussion...".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };

        store.append_event(&event).await.unwrap();

        // Verify summary fields were stored
        let row: (String, Option<String>) = sqlx::query_as(
            "SELECT summary_type, summary_topic FROM session_events WHERE event_type = 'summary'",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert_eq!(row.0, "topic");
        assert_eq!(row.1, Some("discussion about API".to_string()));
    }

    #[tokio::test]
    async fn test_session_auto_created() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        let event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "auto:session".into(),
            parent_id: None,
            event_type: EventType::UserMessage,
            content: "Test".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };

        store.append_event(&event).await.unwrap();

        // Verify session was auto-created
        let count: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM sessions_v2")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(count.0, 1);

        // Verify total_events was incremented
        let total_events: (i32,) =
            sqlx::query_as("SELECT total_events FROM sessions_v2 WHERE key = 'auto:session'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(total_events.0, 1);
    }

    #[tokio::test]
    async fn test_branch_tracking() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        let event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            parent_id: None,
            event_type: EventType::UserMessage,
            content: "Test".into(),
            embedding: None,
            metadata: EventMetadata {
                branch: Some("feature".into()),
                ..Default::default()
            },
            created_at: Utc::now(),
        };

        store.append_event(&event).await.unwrap();

        // Verify branch is tracked in event
        let branch: (String,) =
            sqlx::query_as("SELECT branch FROM session_events WHERE session_key = 'test:session'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(branch.0, "feature");

        // Verify branches JSON was updated
        let branches: (String,) =
            sqlx::query_as("SELECT branches FROM sessions_v2 WHERE key = 'test:session'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert!(branches.0.contains("feature"));
    }

    #[tokio::test]
    async fn test_append_merge_event() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        // First create a source branch with an event
        let source_event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            parent_id: None,
            event_type: EventType::UserMessage,
            content: "Source branch content".into(),
            embedding: None,
            metadata: EventMetadata {
                branch: Some("feature".into()),
                ..Default::default()
            },
            created_at: Utc::now(),
        };
        store.append_event(&source_event).await.unwrap();

        // Now create a merge event
        let merge_event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            parent_id: None,
            event_type: EventType::Merge {
                source_branch: "feature".into(),
                source_head: source_event.id,
            },
            content: "Merged feature branch".into(),
            embedding: None,
            metadata: EventMetadata {
                branch: Some("main".into()),
                ..Default::default()
            },
            created_at: Utc::now(),
        };

        store.append_event(&merge_event).await.unwrap();

        // Verify merge fields were stored
        let row: (String, String, String) = sqlx::query_as(
            "SELECT event_type, merge_source, merge_head FROM session_events WHERE event_type = 'merge'",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert_eq!(row.0, "merge");
        assert_eq!(row.1, "feature");
        assert_eq!(row.2, source_event.id.to_string());
    }

    #[tokio::test]
    async fn test_append_event_with_embedding() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        // Create event with embedding (e.g., 4-dimensional vector)
        let embedding = vec![0.1_f32, 0.2, 0.3, 0.4];
        let event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            parent_id: None,
            event_type: EventType::UserMessage,
            content: "Message with embedding".into(),
            embedding: Some(embedding.clone()),
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };

        store.append_event(&event).await.unwrap();

        // Verify embedding was stored correctly
        let row: (Option<Vec<u8>>,) = sqlx::query_as(
            "SELECT embedding FROM session_events WHERE session_key = 'test:session'",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();

        // Embedding should be stored as bytes (4 floats * 4 bytes = 16 bytes)
        let stored_bytes = row.0.expect("embedding should be stored");
        assert_eq!(stored_bytes.len(), 16); // 4 floats * 4 bytes

        // Verify the embedding values can be reconstructed
        let stored_embedding: Vec<f32> = bytemuck::cast_slice(&stored_bytes).to_vec();
        assert_eq!(stored_embedding, embedding);
    }

    #[tokio::test]
    async fn test_get_branch_history() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        // Add event chain
        let e1 = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            parent_id: None,
            event_type: EventType::UserMessage,
            content: "Hello".into(),
            embedding: None,
            metadata: EventMetadata {
                branch: Some("main".into()),
                ..Default::default()
            },
            created_at: Utc::now(),
        };
        store.append_event(&e1).await.unwrap();

        let e2 = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            parent_id: Some(e1.id),
            event_type: EventType::AssistantMessage,
            content: "Hi!".into(),
            embedding: None,
            metadata: EventMetadata {
                branch: Some("main".into()),
                ..Default::default()
            },
            created_at: Utc::now(),
        };
        store.append_event(&e2).await.unwrap();

        // Read history
        let history = store
            .get_branch_history("test:session", "main")
            .await
            .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content, "Hello");
        assert_eq!(history[1].content, "Hi!");
        assert_eq!(history[1].parent_id, Some(e1.id));
    }

    #[tokio::test]
    async fn test_get_branch_history_empty() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        // Query non-existent session
        let history = store
            .get_branch_history("nonexistent:session", "main")
            .await
            .unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn test_get_branch_history_different_branches() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        // Add event to main branch
        let main_event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            parent_id: None,
            event_type: EventType::UserMessage,
            content: "Main branch".into(),
            embedding: None,
            metadata: EventMetadata {
                branch: Some("main".into()),
                ..Default::default()
            },
            created_at: Utc::now(),
        };
        store.append_event(&main_event).await.unwrap();

        // Add event to feature branch
        let feature_event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            parent_id: None,
            event_type: EventType::UserMessage,
            content: "Feature branch".into(),
            embedding: None,
            metadata: EventMetadata {
                branch: Some("feature".into()),
                ..Default::default()
            },
            created_at: Utc::now(),
        };
        store.append_event(&feature_event).await.unwrap();

        // Query main branch
        let main_history = store
            .get_branch_history("test:session", "main")
            .await
            .unwrap();
        assert_eq!(main_history.len(), 1);
        assert_eq!(main_history[0].content, "Main branch");

        // Query feature branch
        let feature_history = store
            .get_branch_history("test:session", "feature")
            .await
            .unwrap();
        assert_eq!(feature_history.len(), 1);
        assert_eq!(feature_history[0].content, "Feature branch");
    }

    #[tokio::test]
    async fn test_get_branch_history_with_tool_call() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        // Add tool call event
        let tool_call = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            parent_id: None,
            event_type: EventType::ToolCall {
                tool_name: "read_file".into(),
                arguments: serde_json::json!({"path": "/test.txt"}),
            },
            content: "".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };
        store.append_event(&tool_call).await.unwrap();

        // Read and verify
        let history = store
            .get_branch_history("test:session", "main")
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
            _ => panic!("Expected ToolCall event type"),
        }
    }

    #[tokio::test]
    async fn test_get_branch_history_with_summary() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        let covered_ids = vec![Uuid::now_v7(), Uuid::now_v7()];
        let summary = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            parent_id: None,
            event_type: EventType::Summary {
                summary_type: SummaryType::Topic {
                    topic: "API discussion".into(),
                },
                covered_event_ids: covered_ids.clone(),
            },
            content: "Summary content".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };
        store.append_event(&summary).await.unwrap();

        // Read and verify
        let history = store
            .get_branch_history("test:session", "main")
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        match &history[0].event_type {
            EventType::Summary {
                summary_type,
                covered_event_ids,
            } => {
                match summary_type {
                    SummaryType::Topic { topic } => assert_eq!(topic, "API discussion"),
                    _ => panic!("Expected Topic summary type"),
                }
                assert_eq!(covered_event_ids, &covered_ids);
            }
            _ => panic!("Expected Summary event type"),
        }
    }

    #[tokio::test]
    async fn test_get_events_by_ids() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        // Add event chain
        let e1 = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            parent_id: None,
            event_type: EventType::UserMessage,
            content: "Event 1".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };
        store.append_event(&e1).await.unwrap();

        let e2 = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            parent_id: Some(e1.id),
            event_type: EventType::AssistantMessage,
            content: "Event 2".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };
        store.append_event(&e2).await.unwrap();

        let e3 = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            parent_id: Some(e2.id),
            event_type: EventType::UserMessage,
            content: "Event 3".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };
        store.append_event(&e3).await.unwrap();

        // Query specific events
        let events = store
            .get_events_by_ids("test:session", &[e1.id, e3.id])
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].content, "Event 1");
        assert_eq!(events[1].content, "Event 3");

        // Query with non-existent ID
        let events = store
            .get_events_by_ids("test:session", &[Uuid::now_v7()])
            .await
            .unwrap();
        assert!(events.is_empty());

        // Query with empty list
        let events = store.get_events_by_ids("test:session", &[]).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn test_clear_session() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        // Add multiple events to a session
        let e1 = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            parent_id: None,
            event_type: EventType::UserMessage,
            content: "Event 1".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };
        store.append_event(&e1).await.unwrap();

        let e2 = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            parent_id: Some(e1.id),
            event_type: EventType::AssistantMessage,
            content: "Event 2".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };
        store.append_event(&e2).await.unwrap();

        // Verify events exist
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

        // Clear the session
        store.clear_session("test:session").await.unwrap();

        // Verify all events are deleted
        let count: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM session_events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count.0, 0);

        // Verify session record is deleted
        let session_count: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM sessions_v2")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(session_count.0, 0);
    }
}
