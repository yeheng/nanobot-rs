//! Generic key-value storage repository.

/// Repository for raw KV operations on the `kv_store` table.
#[derive(Clone)]
pub struct KvStore {
    pool: sqlx::SqlitePool,
}

impl KvStore {
    /// Create from an existing pool.
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    /// Read a raw string value by key.
    pub async fn read(&self, key: &str) -> anyhow::Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT value FROM kv_store WHERE key = $1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(v,)| v))
    }

    /// Write (or overwrite) a raw string value by key.
    pub async fn write(&self, key: &str, value: &str) -> anyhow::Result<()> {
        sqlx::query("INSERT OR REPLACE INTO kv_store (key, value) VALUES ($1, $2)")
            .bind(key)
            .bind(value)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete a raw key-value entry.
    ///
    /// Returns `true` if a row was actually deleted.
    pub async fn delete(&self, key: &str) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM kv_store WHERE key = $1")
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
