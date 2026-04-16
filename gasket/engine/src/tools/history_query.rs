//! History query tool for searching conversation records in SQLite.
//!
//! Provides a `query_history` tool that searches the `session_messages` table
//! without relying on the external `sqlite3` CLI binary (which may be blocked
//! by macOS sandbox or enterprise policies).

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use sqlx::Row;
use tracing::instrument;

use super::{Tool, ToolContext, ToolError, ToolResult};
use gasket_storage::SqlitePool;

/// Query conversation history from the local SQLite store.
///
/// This tool bypasses the need for an external `sqlite3` binary by using
/// `sqlx` to query the `session_messages` table directly via the async pool.
pub struct HistoryQueryTool {
    pool: SqlitePool,
}

impl HistoryQueryTool {
    /// Create a new history query tool with the given SQLite pool.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

// ── Argument Parsing ───────────────────────────────────────────

#[derive(Deserialize)]
struct QueryArgs {
    /// Optional keywords to search for in message content (case-insensitive LIKE).
    keywords: Option<String>,

    /// Maximum number of messages to return (default: 20).
    limit: Option<usize>,

    /// Optional session key override. If omitted, the current session key from
    /// `ToolContext` is used.
    session_key: Option<String>,
}

#[async_trait]
impl Tool for HistoryQueryTool {
    fn name(&self) -> &str {
        "query_history"
    }

    fn description(&self) -> &str {
        "Query conversation history from the local SQLite database. \
         Supports keyword search and filtering by session. \
         This works even when the sqlite3 CLI is blocked by sandbox policies."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "keywords": {
                    "type": "string",
                    "description": "Optional keywords to search for in message content (case-insensitive)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of messages to return",
                    "default": 20
                },
                "session_key": {
                    "type": "string",
                    "description": "Optional session key (e.g. 'telegram:12345'). Uses current session if omitted."
                }
            },
            "required": []
        })
    }

    #[instrument(name = "tool.query_history", skip_all)]
    async fn execute(&self, params: Value, ctx: &ToolContext) -> ToolResult {
        let args: QueryArgs = serde_json::from_value(params)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let session_key = args
            .session_key
            .unwrap_or_else(|| ctx.session_key.to_string());
        let limit = args.limit.unwrap_or(20).min(100) as i64;
        let pattern = args
            .keywords
            .map(|k| format!("%{}%", k.replace('%', "\\%").replace('_', "\\_")))
            .unwrap_or_else(|| "%".to_string());

        let rows = sqlx::query(
            r#"
            SELECT role, content, timestamp
            FROM session_messages
            WHERE session_key = ?1
              AND content LIKE ?2 ESCAPE '\'
            ORDER BY timestamp DESC
            LIMIT ?3
            "#,
        )
        .bind(&session_key)
        .bind(&pattern)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ToolError::ExecutionError(format!("Database query failed: {}", e)))?;

        if rows.is_empty() {
            return Ok(format!(
                "No history found for session '{}' with the given keywords.",
                session_key
            ));
        }

        let mut lines = vec![format!(
            "Conversation history for session '{}' ({} messages):",
            session_key,
            rows.len()
        )];

        for row in rows {
            let role: String = row.try_get("role").unwrap_or_default();
            let content: String = row.try_get("content").unwrap_or_default();
            let timestamp: String = row.try_get("timestamp").unwrap_or_default();
            let preview = if content.len() > 400 {
                format!("{}...", &content[..400])
            } else {
                content
            };
            lines.push(format!("\n[{}] {}:\n{}", timestamp, role, preview));
        }

        Ok(lines.join(""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gasket_storage::SqliteStore;

    async fn setup_test_db() -> SqlitePool {
        let path = std::env::temp_dir().join(format!(
            "gasket_test_history_query_{}.db",
            uuid::Uuid::new_v4()
        ));
        let store = SqliteStore::with_path(path).await.unwrap();
        // seed a session and a message
        sqlx::query(
            "INSERT OR IGNORE INTO sessions (key, updated_at) VALUES ('cli:test', datetime('now'))",
        )
        .execute(&store.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO session_messages (session_key, role, content, timestamp) VALUES ('cli:test', 'user', 'Hello world', datetime('now'))"
        )
        .execute(&store.pool())
        .await
        .unwrap();
        store.pool()
    }

    #[tokio::test]
    async fn test_history_query_by_keywords() {
        let pool = setup_test_db().await;
        let tool = HistoryQueryTool::new(pool);
        let args = serde_json::json!({
            "keywords": "Hello",
            "limit": 10,
            "session_key": "cli:test"
        });
        let result = tool.execute(args, &ToolContext::default()).await;
        assert!(result.is_ok());
        let text = result.unwrap();
        assert!(text.contains("Hello world"));
        assert!(text.contains("user"));
    }

    #[tokio::test]
    async fn test_history_query_no_results() {
        let pool = setup_test_db().await;
        let tool = HistoryQueryTool::new(pool);
        let args = serde_json::json!({
            "keywords": "nonexistent",
            "session_key": "cli:test"
        });
        let result = tool.execute(args, &ToolContext::default()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("No history found"));
    }
}
