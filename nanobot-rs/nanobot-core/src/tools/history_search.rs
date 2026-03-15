//! History search tool using SQLite database.
//!
//! Provides a `history_search` tool that searches conversation history
//! stored in the SQLite database (`session_messages` table).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use sqlx::Row;
use tracing::debug;

use super::{simple_schema, Tool, ToolError, ToolResult};
use crate::memory::SqliteStore;

// ── History Search Tool ────────────────────────────────────────

/// History search tool using SQLite database.
///
/// Searches conversation history stored in the `session_messages` table.
/// Supports filtering by session, role, and time range.
pub struct HistorySearchTool {
    db: SqliteStore,
    /// Configured default limit for results (from config.yaml or fallback 15)
    default_limit: usize,
}

impl HistorySearchTool {
    /// Create a new history search tool with a SQLite store and configured limit.
    pub fn new(db: SqliteStore, default_limit: usize) -> Self {
        Self { db, default_limit }
    }

    /// Create with default SQLite store.
    pub async fn with_defaults() -> Result<Self, ToolError> {
        let db = SqliteStore::new()
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Failed to open database: {}", e)))?;
        Ok(Self {
            db,
            default_limit: 15,
        })
    }
}

// ── Argument Parsing ───────────────────────────────────────────

#[derive(Deserialize)]
struct SearchArgs {
    /// Search query text (keyword search)
    query: Option<String>,

    /// Filter by session key (e.g., "telegram:123456")
    session_key: Option<String>,

    /// Filter by role (user/assistant/system/tool)
    role: Option<String>,

    /// Start date filter (ISO 8601 format)
    from_date: Option<String>,

    /// End date filter (ISO 8601 format)
    to_date: Option<String>,

    /// Maximum number of results (0 means use configured default)
    #[serde(default)]
    limit: usize,
}

// ── Tool Implementation ────────────────────────────────────────

#[async_trait]
impl Tool for HistorySearchTool {
    fn name(&self) -> &str {
        "history_search"
    }

    fn description(&self) -> &str {
        "Search conversation history from the SQLite database. \
         Find past messages, conversations, or specific discussions. \
         Supports filtering by session, role, and time range."
    }

    fn parameters(&self) -> Value {
        simple_schema(&[
            (
                "query",
                "string",
                false,
                "Search query text (keywords to find in message content)",
            ),
            (
                "session_key",
                "string",
                false,
                "Filter by session key (e.g., 'telegram:123456', 'cli:interactive')",
            ),
            (
                "role",
                "string",
                false,
                "Filter by role: 'user', 'assistant', 'system', or 'tool'",
            ),
            (
                "from_date",
                "string",
                false,
                "Start date filter (ISO 8601 format, e.g., '2024-01-01T00:00:00Z')",
            ),
            (
                "to_date",
                "string",
                false,
                "End date filter (ISO 8601 format, e.g., '2024-12-31T23:59:59Z')",
            ),
            (
                "limit",
                "integer",
                false,
                "Maximum number of results (default: 15)",
            ),
        ])
    }

    async fn execute(&self, args: Value) -> ToolResult {
        let mut parsed: SearchArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("Invalid arguments: {}", e)))?;

        // Apply configured default when caller doesn't specify a limit
        if parsed.limit == 0 {
            parsed.limit = self.default_limit;
        }

        self.search_history(&parsed).await
    }
}

impl HistorySearchTool {
    /// Execute search using SQLite LIKE query.
    async fn search_history(&self, parsed: &SearchArgs) -> ToolResult {
        let mut conditions: Vec<String> = Vec::new();
        let mut param_count = 1;

        // Build WHERE conditions (values are bound later in the same order)
        if parsed.query.is_some() {
            conditions.push(format!("content LIKE ${}", param_count));
            param_count += 1;
        }

        if parsed.session_key.is_some() {
            conditions.push(format!("session_key = ${}", param_count));
            param_count += 1;
        }

        if parsed.role.is_some() {
            conditions.push(format!("role = ${}", param_count));
            param_count += 1;
        }

        if parsed.from_date.is_some() {
            conditions.push(format!("timestamp >= ${}", param_count));
            param_count += 1;
        }

        if parsed.to_date.is_some() {
            conditions.push(format!("timestamp <= ${}", param_count));
            let _ = param_count; // last use — silence unused-assignment warning
        }

        // Build SQL query
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let sql = format!(
            "SELECT session_key, role, content, timestamp \
             FROM session_messages {} \
             ORDER BY timestamp DESC \
             LIMIT {}",
            where_clause, parsed.limit
        );

        debug!("history_search: executing SQL: {}", sql);

        // Build query
        let mut query = sqlx::query(&sql);

        // Bind parameters
        if let Some(ref q) = parsed.query {
            query = query.bind(format!("%{}%", q));
        }

        if let Some(ref sk) = parsed.session_key {
            query = query.bind(sk);
        }

        if let Some(ref r) = parsed.role {
            query = query.bind(r);
        }

        if let Some(ref fd) = parsed.from_date {
            query = query.bind(fd);
        }

        if let Some(ref td) = parsed.to_date {
            query = query.bind(td);
        }

        // Execute query
        let rows: Vec<sqlx::sqlite::SqliteRow> = query
            .fetch_all(&self.db.pool)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Database query failed: {}", e)))?;

        if rows.is_empty() {
            let filter_desc = self.describe_filters(parsed);
            return Ok(format!(
                "No messages found matching criteria: {}",
                filter_desc
            ));
        }

        // Format results
        let mut output = format!(
            "Found {} message{} in history:\n\n",
            rows.len(),
            if rows.len() == 1 { "" } else { "s" }
        );

        for (i, row) in rows.iter().enumerate() {
            let session_key: String = row.get("session_key");
            let role: String = row.get("role");
            let content: String = row.get("content");
            let timestamp_str: String = row.get("timestamp");

            // Parse timestamp
            let timestamp = DateTime::parse_from_rfc3339(&timestamp_str)
                .map(|dt| dt.with_timezone(&Utc).format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|_| timestamp_str.clone());

            output.push_str(&format!(
                "{}. [{}] **{}** ({}):\n",
                i + 1,
                role.to_uppercase(),
                session_key,
                timestamp
            ));
            output.push_str(&format!(
                "   {}\n\n",
                content.clone().trim().replace('\n', " ")
            ));
        }

        Ok(output)
    }

    fn describe_filters(&self, parsed: &SearchArgs) -> String {
        let mut parts = Vec::new();

        if let Some(ref q) = parsed.query {
            parts.push(format!("query='{}'", q));
        }
        if let Some(ref sk) = parsed.session_key {
            parts.push(format!("session='{}'", sk));
        }
        if let Some(ref r) = parsed.role {
            parts.push(format!("role='{}'", r));
        }
        if let Some(ref fd) = parsed.from_date {
            parts.push(format!("from='{}'", fd));
        }
        if let Some(ref td) = parsed.to_date {
            parts.push(format!("to='{}'", td));
        }

        if parts.is_empty() {
            "no filters".to_string()
        } else {
            parts.join(", ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_args_parsing() {
        let args = serde_json::json!({
            "query": "API design",
            "role": "user",
            "limit": 10
        });

        let parsed: SearchArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.query, Some("API design".to_string()));
        assert_eq!(parsed.role, Some("user".to_string()));
        assert_eq!(parsed.limit, 10);
    }

    #[test]
    fn test_search_args_defaults() {
        let args = serde_json::json!({});
        let parsed: SearchArgs = serde_json::from_value(args).unwrap();
        // limit defaults to 0 (meaning "use configured default from ToolsConfig")
        assert_eq!(parsed.limit, 0);
        assert!(parsed.query.is_none());
    }

    #[test]
    fn test_search_args_session_key() {
        let args = serde_json::json!({
            "session_key": "telegram:123456"
        });
        let parsed: SearchArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.session_key, Some("telegram:123456".to_string()));
    }
}
