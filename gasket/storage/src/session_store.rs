//! Session storage repository — summaries, checkpoints, and embeddings.

use gasket_types::SessionKey;
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

    // ── Combined Summary + Checkpoint ──

    /// Load summary with watermark and merge latest checkpoint.
    ///
    /// Returns `(merged_summary, watermark)`.
    /// If no summary exists, returns `("", 0)`.
    pub async fn load_summary_with_checkpoint(
        &self,
        session_key: &SessionKey,
    ) -> anyhow::Result<(String, i64)> {
        let (mut summary, watermark) = match self.load_summary(session_key).await {
            Ok(Some((content, watermark))) => (content, watermark),
            Ok(None) => (String::new(), 0),
            Err(e) => return Err(e),
        };

        let key_str = session_key.to_string();
        if let Ok(Some((ck_summary, _ck_seq))) = self.load_checkpoint(&key_str, i64::MAX).await {
            if !ck_summary.is_empty() {
                if !summary.is_empty() {
                    summary.push_str("\n\n[Working Memory]\n");
                }
                summary.push_str(&ck_summary);
            }
        }

        Ok((summary, watermark))
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

    // ── Compaction State API ──

    /// Mark that compaction has started for a session.
    pub async fn mark_compaction_started(&self, session_key: &SessionKey) -> anyhow::Result<()> {
        let key_str = session_key.to_string();
        let started_at = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT OR REPLACE INTO session_summaries (session_key, content, covered_upto_sequence, created_at, compaction_in_progress, compaction_started_at)
             VALUES ($1, COALESCE((SELECT content FROM session_summaries WHERE session_key = $1), ''), COALESCE((SELECT covered_upto_sequence FROM session_summaries WHERE session_key = $1), 0), COALESCE((SELECT created_at FROM session_summaries WHERE session_key = $1), datetime('now')), 1, $2)",
        )
        .bind(&key_str)
        .bind(&started_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Mark that compaction has finished for a session.
    pub async fn mark_compaction_finished(&self, session_key: &SessionKey) -> anyhow::Result<()> {
        let key_str = session_key.to_string();
        sqlx::query(
            "UPDATE session_summaries SET compaction_in_progress = 0, compaction_started_at = NULL WHERE session_key = $1",
        )
        .bind(&key_str)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Check whether compaction is marked as in-progress for a session.
    pub async fn is_compaction_in_progress(
        &self,
        session_key: &SessionKey,
    ) -> anyhow::Result<bool> {
        let key_str = session_key.to_string();
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT compaction_in_progress FROM session_summaries WHERE session_key = $1",
        )
        .bind(&key_str)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(v,)| v != 0).unwrap_or(false))
    }
}
