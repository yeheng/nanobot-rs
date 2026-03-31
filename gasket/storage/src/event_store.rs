//! Event store for event sourcing architecture.

use chrono::{DateTime, Utc};
use gasket_types::{EventMetadata, EventType, SessionEvent, TokenUsage};
use serde::{Deserialize, Serialize};
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

pub struct EventStore {
    pool: SqlitePool,
}

impl EventStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn append_event(&self, event: &SessionEvent) -> Result<(), StoreError> {
        let event_type_tag = event_type_tag(&event.event_type);
        let event_data = EventData::from_event_type(&event.event_type);
        let event_data_json = event_data
            .as_ref()
            .map(|d| serde_json::to_string(d))
            .transpose()?;
        let tools_used = serde_json::to_string(&event.metadata.tools_used)?;
        let token_usage = event
            .metadata
            .token_usage
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let extra = serde_json::to_string(&event.metadata.extra)?;

        let mut tx = self.pool.begin().await?;

        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT OR IGNORE INTO sessions_v2 (key, created_at, updated_at) VALUES (?, ?, ?)",
        )
        .bind(&event.session_key)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO session_events
            (id, session_key, event_type, content, embedding, branch,
             tools_used, token_usage, event_data, extra, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(event.id.to_string())
        .bind(&event.session_key)
        .bind(event_type_tag)
        .bind(&event.content)
        .bind(
            event
                .embedding
                .as_ref()
                .map(|e| bytemuck::cast_slice(e) as &[u8]),
        )
        .bind(event.metadata.branch.as_deref().unwrap_or("main"))
        .bind(&tools_used)
        .bind(token_usage.as_deref())
        .bind(event_data_json.as_deref())
        .bind(&extra)
        .bind(event.created_at.to_rfc3339())
        .execute(&mut *tx)
        .await?;

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

        tx.commit().await?;
        Ok(())
    }

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

    pub async fn clear_session(&self, session_key: &str) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM session_events WHERE session_key = ?")
            .bind(session_key)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM sessions_v2 WHERE key = ?")
            .bind(session_key)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct EventRow {
    id: String,
    session_key: String,
    event_type: String,
    content: String,
    embedding: Option<Vec<u8>>,
    branch: String,
    tools_used: String,
    token_usage: Option<String>,
    event_data: Option<String>,
    extra: String,
    created_at: String,
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
        let embedding = row.embedding.map(|b| bytemuck::cast_slice(&b).to_vec());

        Ok(SessionEvent {
            id: row
                .id
                .parse()
                .map_err(|_| StoreError::InvalidUuid(row.id.clone()))?,
            session_key: row.session_key,
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
                event_type TEXT NOT NULL,
                content TEXT NOT NULL,
                embedding BLOB,
                branch TEXT DEFAULT 'main',
                tools_used TEXT DEFAULT '[]',
                token_usage TEXT,
                event_data TEXT,
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
            event_type: EventType::UserMessage,
            content: "Hello, world!".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
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
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };

        store.append_event(&event).await.unwrap();

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
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };

        store.append_event(&event).await.unwrap();

        let history = store
            .get_branch_history("test:session", "main")
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
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };

        store.append_event(&event).await.unwrap();

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
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
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
    async fn test_branch_tracking() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        let event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
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

        let branch: (String,) =
            sqlx::query_as("SELECT branch FROM session_events WHERE session_key = 'test:session'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(branch.0, "feature");

        let branches: (String,) =
            sqlx::query_as("SELECT branches FROM sessions_v2 WHERE key = 'test:session'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert!(branches.0.contains("feature"));
    }

    #[tokio::test]
    async fn test_append_event_with_embedding() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        let embedding = vec![0.1_f32, 0.2, 0.3, 0.4];
        let event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::UserMessage,
            content: "Message with embedding".into(),
            embedding: Some(embedding.clone()),
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };

        store.append_event(&event).await.unwrap();

        let row: (Option<Vec<u8>>,) = sqlx::query_as(
            "SELECT embedding FROM session_events WHERE session_key = 'test:session'",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();

        let stored_bytes = row.0.expect("embedding should be stored");
        assert_eq!(stored_bytes.len(), 16);

        let stored_embedding: Vec<f32> = bytemuck::cast_slice(&stored_bytes).to_vec();
        assert_eq!(stored_embedding, embedding);
    }

    #[tokio::test]
    async fn test_get_branch_history() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

        let e1 = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
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

        let history = store
            .get_branch_history("test:session", "main")
            .await
            .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content, "Hello");
        assert_eq!(history[1].content, "Hi!");
    }

    #[tokio::test]
    async fn test_get_branch_history_empty() {
        let pool = setup_test_db().await;
        let store = EventStore::new(pool);

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

        let main_event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
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

        let feature_event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
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

        let main_history = store
            .get_branch_history("test:session", "main")
            .await
            .unwrap();
        assert_eq!(main_history.len(), 1);
        assert_eq!(main_history[0].content, "Main branch");

        let feature_history = store
            .get_branch_history("test:session", "feature")
            .await
            .unwrap();
        assert_eq!(feature_history.len(), 1);
        assert_eq!(feature_history[0].content, "Feature branch");
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
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };
        store.append_event(&e1).await.unwrap();

        let e2 = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
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
            event_type: EventType::UserMessage,
            content: "Event 3".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };
        store.append_event(&e3).await.unwrap();

        let events = store
            .get_events_by_ids("test:session", &[e1.id, e3.id])
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].content, "Event 1");
        assert_eq!(events[1].content, "Event 3");

        let events = store
            .get_events_by_ids("test:session", &[Uuid::now_v7()])
            .await
            .unwrap();
        assert!(events.is_empty());

        let events = store.get_events_by_ids("test:session", &[]).await.unwrap();
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
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };
        store.append_event(&e1).await.unwrap();

        let e2 = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            event_type: EventType::AssistantMessage,
            content: "Event 2".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
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

        store.clear_session("test:session").await.unwrap();

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
