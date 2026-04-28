//! SQLite-backed embedding persistence.

use anyhow::{anyhow, Result};
use sqlx::{Row, SqlitePool};

/// An embedding record stored in SQLite.
pub struct StoredEmbedding {
    pub event_id: String,
    pub session_key: String,
    pub embedding: Vec<f32>,
    pub event_type: String,
    pub created_at: String,
}

/// Store for persisting embeddings in SQLite.
pub struct EmbeddingStore {
    pool: SqlitePool,
    dim: usize,
}

impl EmbeddingStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool, dim: 0 }
    }

    pub fn with_dim(pool: SqlitePool, dim: usize) -> Self {
        Self { pool, dim }
    }

    /// Creates the event_embeddings table and indexes if they do not exist.
    pub async fn run_migration(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS event_embeddings (
                event_id     TEXT PRIMARY KEY,
                session_key  TEXT NOT NULL,
                channel      TEXT NOT NULL DEFAULT '',
                chat_id      TEXT NOT NULL DEFAULT '',
                embedding    BLOB NOT NULL,
                dim          INTEGER NOT NULL,
                event_type   TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                created_at   TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_emb_session ON event_embeddings(session_key)")
            .execute(&self.pool)
            .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_emb_channel_chat ON event_embeddings(channel, chat_id)",
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Insert a single embedding. Uses INSERT OR IGNORE for idempotency.
    #[allow(clippy::too_many_arguments)]
    pub async fn save(
        &self,
        event_id: &str,
        session_key: &str,
        channel: &str,
        chat_id: &str,
        embedding: &[f32],
        event_type: &str,
        content_hash: &str,
    ) -> Result<()> {
        let blob = embedding_to_bytes(embedding);
        let dim = embedding.len() as i32;
        let now = chrono::Utc::now().to_rfc3339();

        sqlx::query(
            r#"
            INSERT OR IGNORE INTO event_embeddings
                (event_id, session_key, channel, chat_id, embedding, dim, event_type, content_hash, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(event_id)
        .bind(session_key)
        .bind(channel)
        .bind(chat_id)
        .bind(&blob[..])
        .bind(dim)
        .bind(event_type)
        .bind(content_hash)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Transactional batch insert.
    pub async fn save_batch(&self, items: &[EmbeddingInput<'_>]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;
        let now = chrono::Utc::now().to_rfc3339();

        for item in items {
            let blob = embedding_to_bytes(item.embedding);
            let dim = item.embedding.len() as i32;

            sqlx::query(
                r#"
                INSERT OR IGNORE INTO event_embeddings
                    (event_id, session_key, channel, chat_id, embedding, dim, event_type, content_hash, created_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(item.event_id)
            .bind(item.session_key)
            .bind(item.channel)
            .bind(item.chat_id)
            .bind(&blob[..])
            .bind(dim)
            .bind(item.event_type)
            .bind(item.content_hash)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Load all embeddings (for cold-start index rebuild).
    pub async fn load_all(&self) -> Result<Vec<StoredEmbedding>> {
        let rows = sqlx::query(
            "SELECT event_id, session_key, embedding, event_type, created_at FROM event_embeddings",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut results = Vec::with_capacity(rows.len());
        for row in rows {
            let event_id: String = row.get("event_id");
            let session_key: String = row.get("session_key");
            let blob: Vec<u8> = row.get("embedding");
            let embedding = match bytes_to_embedding(&blob) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        "skipping corrupted embedding for event_id={}: {}",
                        event_id,
                        e
                    );
                    continue;
                }
            };
            results.push(StoredEmbedding {
                event_id,
                session_key,
                embedding,
                event_type: row.get("event_type"),
                created_at: row.get("created_at"),
            });
        }

        Ok(results)
    }

    /// Delete embeddings by event IDs. Returns number of deleted rows.
    pub async fn delete_by_event_ids(&self, ids: &[String]) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }

        let placeholders: Vec<&str> = ids.iter().map(|_| "?").collect();
        let sql = format!(
            "DELETE FROM event_embeddings WHERE event_id IN ({})",
            placeholders.join(",")
        );

        let mut query = sqlx::query(&sql);
        for id in ids {
            query = query.bind(id);
        }

        let result = query.execute(&self.pool).await?;
        Ok(result.rows_affected())
    }

    /// Delete all embeddings for a session. Returns number of deleted rows.
    pub async fn delete_by_session(&self, session_key: &str) -> Result<u64> {
        let result = sqlx::query("DELETE FROM event_embeddings WHERE session_key = ?")
            .bind(session_key)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected())
    }

    /// Check if an embedding exists for the given event ID.
    pub async fn exists(&self, event_id: &str) -> Result<bool> {
        let row: Option<(i32,)> =
            sqlx::query_as("SELECT 1 FROM event_embeddings WHERE event_id = ?")
                .bind(event_id)
                .fetch_optional(&self.pool)
                .await?;

        Ok(row.is_some())
    }

    /// Count total embeddings.
    pub async fn count(&self) -> Result<i64> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM event_embeddings")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    /// Load recent embeddings ordered by created_at DESC.
    pub async fn load_recent(&self, limit: usize) -> Result<Vec<StoredEmbedding>> {
        let rows = sqlx::query(
            "SELECT event_id, session_key, embedding, event_type, created_at FROM event_embeddings ORDER BY created_at DESC LIMIT ?",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut results = Vec::with_capacity(rows.len());
        for row in rows {
            let event_id: String = row.get("event_id");
            let session_key: String = row.get("session_key");
            let blob: Vec<u8> = row.get("embedding");
            let embedding = match bytes_to_embedding(&blob) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        "skipping corrupted embedding for event_id={}: {}",
                        event_id,
                        e
                    );
                    continue;
                }
            };
            results.push(StoredEmbedding {
                event_id,
                session_key,
                embedding,
                event_type: row.get("event_type"),
                created_at: row.get("created_at"),
            });
        }

        Ok(results)
    }

    /// Scan all embeddings from SQLite in batches and compute similarity on-the-fly.
    /// Returns top-k results with score >= min_score, excluding given ids.
    pub async fn search_similar(
        &self,
        query: &[f32],
        top_k: usize,
        min_score: f32,
        exclude: &std::collections::HashSet<String>,
    ) -> Result<Vec<(String, f32)>> {
        const BATCH_SIZE: i64 = 1000;
        let mut results: Vec<(String, f32)> = Vec::with_capacity(top_k);
        let mut offset: i64 = 0;

        loop {
            let rows =
                sqlx::query("SELECT event_id, embedding FROM event_embeddings LIMIT ? OFFSET ?")
                    .bind(BATCH_SIZE)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await?;

            if rows.is_empty() {
                break;
            }

            for row in rows {
                let event_id: String = row.get("event_id");
                if exclude.contains(&event_id) {
                    continue;
                }
                let blob: Vec<u8> = row.get("embedding");
                let embedding = match bytes_to_embedding(&blob) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(
                            "skipping corrupted embedding for event_id={}: {}",
                            event_id,
                            e
                        );
                        continue;
                    }
                };
                let sim = crate::index::cosine_similarity(query, &embedding);
                if sim >= min_score {
                    if results.len() < top_k {
                        results.push((event_id, sim));
                    } else {
                        // Replace the weakest candidate if this one is better.
                        let min_idx = results
                            .iter()
                            .enumerate()
                            .min_by(|(_, a), (_, b)| {
                                a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
                            })
                            .map(|(i, _)| i)
                            .unwrap();
                        if sim > results[min_idx].1 {
                            results[min_idx] = (event_id, sim);
                        }
                    }
                }
            }

            offset += BATCH_SIZE;
        }

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(results)
    }
}

/// Input for batch embedding insert.
pub struct EmbeddingInput<'a> {
    pub event_id: &'a str,
    pub session_key: &'a str,
    pub channel: &'a str,
    pub chat_id: &'a str,
    pub embedding: &'a [f32],
    pub event_type: &'a str,
    pub content_hash: &'a str,
}

/// Serialize f32 slice to raw bytes (little-endian on all platforms since f32 is IEEE 754).
fn embedding_to_bytes(v: &[f32]) -> Vec<u8> {
    let byte_count = v.len() * 4;
    let bytes = unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, byte_count) };
    bytes.to_vec()
}

/// Deserialize raw bytes back to Vec<f32>.
fn bytes_to_embedding(bytes: &[u8]) -> anyhow::Result<Vec<f32>> {
    if !bytes.len().is_multiple_of(4) {
        return Err(anyhow!(
            "embedding blob length {} is not a multiple of 4",
            bytes.len()
        ));
    }
    let mut result = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        let arr: [u8; 4] = chunk
            .try_into()
            .map_err(|_| anyhow!("unexpected chunk size in embedding blob"))?;
        result.push(f32::from_le_bytes(arr));
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// VectorStore trait implementation
// ---------------------------------------------------------------------------
#[async_trait::async_trait]
impl crate::vector_store::VectorStore for EmbeddingStore {
    async fn upsert(&self, records: Vec<crate::vector_store::VectorRecord>) -> anyhow::Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let items: Vec<EmbeddingInput<'_>> = records
            .iter()
            .map(|r| EmbeddingInput {
                event_id: &r.id,
                session_key: &r.session_key,
                channel: "",
                chat_id: "",
                embedding: &r.vector,
                event_type: &r.event_type,
                content_hash: &r.content_hash,
            })
            .collect();
        self.save_batch(&items).await
    }

    async fn search(
        &self,
        query: &[f32],
        top_k: usize,
        min_score: f32,
        exclude: &std::collections::HashSet<String>,
    ) -> anyhow::Result<Vec<crate::vector_store::SearchResult>> {
        let raw = self
            .search_similar(query, top_k, min_score, exclude)
            .await?;
        Ok(raw
            .into_iter()
            .map(|(id, score)| crate::vector_store::SearchResult { id, score })
            .collect())
    }

    async fn delete(&self, ids: &[String]) -> anyhow::Result<u64> {
        self.delete_by_event_ids(ids).await
    }

    async fn exists(&self, id: &str) -> anyhow::Result<bool> {
        self.exists(id).await
    }

    async fn count(&self) -> anyhow::Result<i64> {
        self.count().await
    }

    fn dim(&self) -> usize {
        self.dim
    }

    async fn load_all(&self) -> anyhow::Result<Vec<crate::vector_store::StoredEmbedding>> {
        let raw = self.load_all().await?;
        Ok(raw
            .into_iter()
            .map(|e| crate::vector_store::StoredEmbedding {
                event_id: e.event_id,
                session_key: e.session_key,
                embedding: e.embedding,
                event_type: e.event_type,
                created_at: e.created_at,
            })
            .collect())
    }

    async fn load_recent(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<crate::vector_store::StoredEmbedding>> {
        let raw = self.load_recent(limit).await?;
        Ok(raw
            .into_iter()
            .map(|e| crate::vector_store::StoredEmbedding {
                event_id: e.event_id,
                session_key: e.session_key,
                embedding: e.embedding,
                event_type: e.event_type,
                created_at: e.created_at,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn test_store() -> EmbeddingStore {
        let pool = SqlitePoolOptions::new()
            .connect(":memory:")
            .await
            .expect("in-memory pool");
        let store = EmbeddingStore::new(pool);
        store.run_migration().await.expect("migration");
        store
    }

    fn sample_embedding() -> Vec<f32> {
        vec![0.1, 0.2, 0.3, 0.4]
    }

    #[tokio::test]
    async fn test_save_and_exists() {
        let store = test_store().await;
        assert!(!store.exists("evt-1").await.unwrap());

        store
            .save(
                "evt-1",
                "sess-a",
                "tg",
                "chat-1",
                &sample_embedding(),
                "user_message",
                "hash1",
            )
            .await
            .unwrap();

        assert!(store.exists("evt-1").await.unwrap());
    }

    #[tokio::test]
    async fn test_load_all() {
        let store = test_store().await;

        let emb1 = vec![0.1, 0.2];
        let emb2 = vec![0.3, 0.4];

        store
            .save("evt-1", "sess-a", "", "", &emb1, "user_message", "h1")
            .await
            .unwrap();
        store
            .save("evt-2", "sess-a", "", "", &emb2, "assistant_message", "h2")
            .await
            .unwrap();

        let all = store.load_all().await.unwrap();
        assert_eq!(all.len(), 2);

        // Verify embedding round-trip accuracy.
        let e1 = all.iter().find(|e| e.event_id == "evt-1").unwrap();
        assert_eq!(e1.embedding.len(), 2);
        assert!((e1.embedding[0] - 0.1f32).abs() < f32::EPSILON);
        assert!((e1.embedding[1] - 0.2f32).abs() < f32::EPSILON);

        let e2 = all.iter().find(|e| e.event_id == "evt-2").unwrap();
        assert_eq!(e2.event_type, "assistant_message");
    }

    #[tokio::test]
    async fn test_delete_by_event_ids() {
        let store = test_store().await;

        store
            .save(
                "evt-1",
                "sess-a",
                "",
                "",
                &sample_embedding(),
                "user_message",
                "h1",
            )
            .await
            .unwrap();
        store
            .save(
                "evt-2",
                "sess-a",
                "",
                "",
                &sample_embedding(),
                "user_message",
                "h2",
            )
            .await
            .unwrap();

        let deleted = store
            .delete_by_event_ids(&["evt-1".to_string()])
            .await
            .unwrap();
        assert_eq!(deleted, 1);

        assert!(!store.exists("evt-1").await.unwrap());
        assert!(store.exists("evt-2").await.unwrap());
    }

    #[tokio::test]
    async fn test_delete_by_session() {
        let store = test_store().await;

        store
            .save(
                "evt-1",
                "sess-a",
                "",
                "",
                &sample_embedding(),
                "user_message",
                "h1",
            )
            .await
            .unwrap();
        store
            .save(
                "evt-2",
                "sess-b",
                "",
                "",
                &sample_embedding(),
                "user_message",
                "h2",
            )
            .await
            .unwrap();

        let deleted = store.delete_by_session("sess-a").await.unwrap();
        assert_eq!(deleted, 1);

        assert!(!store.exists("evt-1").await.unwrap());
        assert!(store.exists("evt-2").await.unwrap());
    }

    #[tokio::test]
    async fn test_save_idempotent() {
        let store = test_store().await;

        store
            .save(
                "evt-1",
                "sess-a",
                "",
                "",
                &sample_embedding(),
                "user_message",
                "h1",
            )
            .await
            .unwrap();
        store
            .save(
                "evt-1",
                "sess-a",
                "",
                "",
                &sample_embedding(),
                "user_message",
                "h1",
            )
            .await
            .unwrap();

        let all = store.load_all().await.unwrap();
        assert_eq!(all.len(), 1, "duplicate insert should produce only 1 row");
    }

    #[tokio::test]
    async fn test_load_all_skips_corrupted_embedding() {
        let store = test_store().await;

        let good = vec![0.1f32, 0.2f32];
        let bad: Vec<u8> = vec![0x01, 0x02, 0x03]; // length 3, not multiple of 4

        store
            .save("evt-good", "sess-a", "", "", &good, "user_message", "h1")
            .await
            .unwrap();

        // Directly insert corrupted blob
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO event_embeddings
                (event_id, session_key, channel, chat_id, embedding, dim, event_type, content_hash, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind("evt-bad")
        .bind("sess-a")
        .bind("")
        .bind("")
        .bind(&bad[..])
        .bind(0i32)
        .bind("user_message")
        .bind("h2")
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&store.pool)
        .await
        .unwrap();

        let all = store.load_all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].event_id, "evt-good");
    }
}
