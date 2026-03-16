//! Key-value store API for SqliteStore.

use chrono::Utc;
use tracing::debug;

use super::SqliteStore;

impl SqliteStore {
    /// Read a raw value by key.
    pub async fn read_raw(&self, key: &str) -> anyhow::Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT value FROM kv_store WHERE key = $1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(v,)| v))
    }

    /// Write a raw value by key (upsert).
    pub async fn write_raw(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let updated_at = Utc::now().to_rfc3339();
        sqlx::query("INSERT OR REPLACE INTO kv_store (key, value, updated_at) VALUES ($1, $2, $3)")
            .bind(key)
            .bind(value)
            .bind(&updated_at)
            .execute(&self.pool)
            .await?;
        debug!("Wrote kv_store key: {}", key);
        Ok(())
    }

    /// Delete a raw key. Returns `true` if the key existed.
    pub async fn delete_raw(&self, key: &str) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM kv_store WHERE key = $1")
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
