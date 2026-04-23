//! Session storage repository — summaries, checkpoints, and embeddings.

use gasket_types::SessionKey;
use sqlx::Row;
use tracing::debug;

/// Repository for session-related SQLite operations.
///
/// Covers: session summaries, checkpoints, and semantic embeddings.
/// All operations are pure SQL — no file-system side effects.
#[derive(Clone)]
pub struct SessionStore {
    pool: sqlx::SqlitePool,
}

impl SessionStore {
    /// Create from an existing pool.
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    // ── Session Summary API ──

    /// Save or replace a session summary with its sequence watermark (upsert).
    pub async fn save_summary(
        &self,
        session_key: &SessionKey,
        content: &str,
        covered_upto_sequence: i64,
    ) -> anyhow::Result<()> {
        let key_str = session_key.to_string();
        let created_at = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT OR REPLACE INTO session_summaries (session_key, content, covered_upto_sequence, created_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(&key_str)
        .bind(content)
        .bind(covered_upto_sequence)
        .bind(&created_at)
        .execute(&self.pool)
        .await?;
        debug!(
            "Saved session summary for {}: covering up to sequence {}",
            session_key, covered_upto_sequence
        );
        Ok(())
    }

    /// Load a session summary and its sequence watermark.
    pub async fn load_summary(
        &self,
        session_key: &SessionKey,
    ) -> anyhow::Result<Option<(String, i64)>> {
        let key_str = session_key.to_string();
        let row: Option<(String, i64)> = sqlx::query_as(
            "SELECT content, covered_upto_sequence FROM session_summaries WHERE session_key = $1",
        )
        .bind(&key_str)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Delete a session summary.
    pub async fn delete_summary(&self, session_key: &SessionKey) -> anyhow::Result<bool> {
        let key_str = session_key.to_string();
        let result = sqlx::query("DELETE FROM session_summaries WHERE session_key = $1")
            .bind(&key_str)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ── Session Checkpoints API ──

    /// Save a checkpoint summary for a session at a specific target_sequence.
    pub async fn save_checkpoint(
        &self,
        session_key: &str,
        target_sequence: i64,
        summary: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO session_checkpoints (session_key, target_sequence, summary, created_at)
             VALUES (?1, ?2, ?3, datetime('now'))",
        )
        .bind(session_key)
        .bind(target_sequence)
        .bind(summary)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Load the most recent checkpoint for a session before or at a given target_sequence.
    pub async fn load_checkpoint(
        &self,
        session_key: &str,
        target_sequence: i64,
    ) -> anyhow::Result<Option<(String, i64)>> {
        let row: Option<(String, i64)> = sqlx::query_as(
            "SELECT summary, target_sequence FROM session_checkpoints
             WHERE session_key = ?1 AND target_sequence <= ?2
             ORDER BY target_sequence DESC
             LIMIT 1",
        )
        .bind(session_key)
        .bind(target_sequence)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    // ── Session Embeddings API ──

    /// Save an embedding for a message.
    pub async fn save_embedding(
        &self,
        message_id: &str,
        session_key: &str,
        embedding: &[f32],
    ) -> anyhow::Result<()> {
        let embedding_bytes = bytemuck::cast_slice(embedding);
        sqlx::query(
            "INSERT OR REPLACE INTO session_embeddings (message_id, session_key, embedding) VALUES ($1, $2, $3)",
        )
        .bind(message_id)
        .bind(session_key)
        .bind(embedding_bytes)
        .execute(&self.pool)
        .await?;
        debug!("Saved embedding for message: {}", message_id);
        Ok(())
    }

    /// Load all embeddings for a session.
    pub async fn load_embeddings(
        &self,
        session_key: &str,
    ) -> anyhow::Result<Vec<(String, String, Vec<f32>)>> {
        let rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query(
            r#"
            SELECT e.message_id, m.content, e.embedding
            FROM session_embeddings e
            LEFT JOIN session_events m ON e.message_id = m.id
            WHERE e.session_key = $1
            ORDER BY m.sequence ASC
            "#,
        )
        .bind(session_key)
        .fetch_all(&self.pool)
        .await?;

        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let message_id: String = row.get("message_id");
            let content: String = row.get("content");
            let embedding_blob: Vec<u8> = row.get("embedding");
            let embedding = bytemuck::cast_slice(&embedding_blob).to_vec();
            result.push((message_id, content, embedding));
        }
        Ok(result)
    }

    /// Check whether an embedding already exists for a given message.
    pub async fn has_embedding(&self, message_id: &str) -> anyhow::Result<bool> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT COUNT(*) FROM session_embeddings WHERE message_id = $1")
                .bind(message_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(count,)| count > 0).unwrap_or(false))
    }

    // ── Evolution / Maintenance helpers ──

    /// Scan all sessions that have at least one event.
    /// Returns `(session_key, total_events)` tuples.
    pub async fn scan_active_sessions(&self) -> anyhow::Result<Vec<(String, i64)>> {
        let rows: Vec<(String, i64)> =
            sqlx::query_as("SELECT key, total_events FROM sessions_v2 WHERE total_events > 0")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    /// Access the underlying pool (for raw queries or `EventStore` construction).
    pub fn pool(&self) -> sqlx::SqlitePool {
        self.pool.clone()
    }
}
