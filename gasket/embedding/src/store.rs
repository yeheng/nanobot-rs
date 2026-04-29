//! SQLite-backed embedding persistence.

use anyhow::{anyhow, Result};
use futures::TryStreamExt;
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
    /// Create a store. `dim` must match the embedding provider's dimension.
    pub fn new(pool: SqlitePool, dim: usize) -> Self {
        assert!(dim > 0, "EmbeddingStore dim must be > 0");
        Self { pool, dim }
    }

    /// Backward-compatible alias for [`EmbeddingStore::new`].
    pub fn with_dim(pool: SqlitePool, dim: usize) -> Self {
        Self::new(pool, dim)
    }

    /// Creates the event_embeddings table and indexes if they do not exist.
    ///
    /// `channel`/`chat_id` are kept in the schema for backward compatibility
    /// with already-deployed databases, but new inserts let them default to
    /// `''` and the unused composite index is dropped.
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

        // Drop the legacy index that was never populated with non-empty values.
        sqlx::query("DROP INDEX IF EXISTS idx_emb_channel_chat")
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Insert a single embedding. Uses INSERT OR IGNORE for idempotency.
    pub async fn save(
        &self,
        event_id: &str,
        session_key: &str,
        embedding: &[f32],
        event_type: &str,
        content_hash: &str,
    ) -> Result<()> {
        if embedding.len() != self.dim {
            return Err(anyhow!(
                "embedding length {} does not match store dim {}",
                embedding.len(),
                self.dim
            ));
        }
        let blob = embedding_to_bytes(embedding);
        let dim = embedding.len() as i32;
        let now = chrono::Utc::now().to_rfc3339();

        sqlx::query(
            r#"
            INSERT OR IGNORE INTO event_embeddings
                (event_id, session_key, embedding, dim, event_type, content_hash, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(event_id)
        .bind(session_key)
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
            if item.embedding.len() != self.dim {
                return Err(anyhow!(
                    "embedding length {} does not match store dim {}",
                    item.embedding.len(),
                    self.dim
                ));
            }
            let blob = embedding_to_bytes(item.embedding);
            let dim = item.embedding.len() as i32;

            sqlx::query(
                r#"
                INSERT OR IGNORE INTO event_embeddings
                    (event_id, session_key, embedding, dim, event_type, content_hash, created_at)
                VALUES (?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(item.event_id)
            .bind(item.session_key)
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

    /// Delete embeddings by event IDs in chunks of 500 to stay below
    /// SQLite's `SQLITE_MAX_VARIABLE_NUMBER` (default 999). Returns total
    /// number of deleted rows.
    pub async fn delete_by_event_ids(&self, ids: &[String]) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        const CHUNK: usize = 500;
        let mut total: u64 = 0;
        for chunk in ids.chunks(CHUNK) {
            let placeholders: Vec<&str> = chunk.iter().map(|_| "?").collect();
            let sql = format!(
                "DELETE FROM event_embeddings WHERE event_id IN ({})",
                placeholders.join(",")
            );

            let mut query = sqlx::query(&sql);
            for id in chunk {
                query = query.bind(id);
            }
            let result = query.execute(&self.pool).await?;
            total += result.rows_affected();
        }
        Ok(total)
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

    /// Stream all embeddings from SQLite and compute similarity on-the-fly.
    /// Returns top-k results with `score >= min_score`, excluding `exclude` ids.
    ///
    /// Uses a streaming `fetch` (not `LIMIT/OFFSET` paging) so cost is O(n)
    /// regardless of table size, and a fixed-size top-k buffer to avoid
    /// allocating a Vec per row.
    pub async fn search_similar(
        &self,
        query: &[f32],
        top_k: usize,
        min_score: f32,
        exclude: &std::collections::HashSet<String>,
    ) -> Result<Vec<(String, f32)>> {
        if top_k == 0 || query.is_empty() {
            return Ok(Vec::new());
        }

        let mut top: Vec<(String, f32)> = Vec::with_capacity(top_k);
        let mut buf: Vec<f32> = Vec::with_capacity(query.len());

        let mut stream =
            sqlx::query("SELECT event_id, embedding FROM event_embeddings").fetch(&self.pool);

        while let Some(row) = stream.try_next().await? {
            let event_id: String = row.get("event_id");
            if exclude.contains(&event_id) {
                continue;
            }
            let blob: Vec<u8> = row.get("embedding");
            buf.clear();
            if let Err(e) = bytes_to_embedding_into(&blob, &mut buf) {
                tracing::warn!(
                    "skipping corrupted embedding for event_id={}: {}",
                    event_id,
                    e
                );
                continue;
            }
            let sim = crate::index::cosine_similarity(query, &buf);
            if sim < min_score {
                continue;
            }
            push_topk(&mut top, top_k, event_id, sim);
        }

        top.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(top)
    }
}

/// Maintain a top-k vector by dropping the weakest candidate when full.
fn push_topk(top: &mut Vec<(String, f32)>, k: usize, id: String, score: f32) {
    if top.len() < k {
        top.push((id, score));
        return;
    }
    let (min_idx, min_score) = top
        .iter()
        .enumerate()
        .map(|(i, (_, s))| (i, *s))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap();
    if score > min_score {
        top[min_idx] = (id, score);
    }
}

/// Input for batch embedding insert.
pub struct EmbeddingInput<'a> {
    pub event_id: &'a str,
    pub session_key: &'a str,
    pub embedding: &'a [f32],
    pub event_type: &'a str,
    pub content_hash: &'a str,
}

/// Serialize f32 slice to little-endian raw bytes.
fn embedding_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// Deserialize raw bytes back to Vec<f32>.
fn bytes_to_embedding(bytes: &[u8]) -> anyhow::Result<Vec<f32>> {
    let mut out = Vec::with_capacity(bytes.len() / 4);
    bytes_to_embedding_into(bytes, &mut out)?;
    Ok(out)
}

/// Deserialize raw bytes into a caller-provided buffer (cleared first).
fn bytes_to_embedding_into(bytes: &[u8], out: &mut Vec<f32>) -> anyhow::Result<()> {
    if !bytes.len().is_multiple_of(4) {
        return Err(anyhow!(
            "embedding blob length {} is not a multiple of 4",
            bytes.len()
        ));
    }
    out.clear();
    out.reserve(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        let arr: [u8; 4] = chunk
            .try_into()
            .map_err(|_| anyhow!("unexpected chunk size in embedding blob"))?;
        out.push(f32::from_le_bytes(arr));
    }
    Ok(())
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
        let store = EmbeddingStore::new(pool, 4);
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
                &sample_embedding(),
                "user_message",
                "hash1",
            )
            .await
            .unwrap();

        assert!(store.exists("evt-1").await.unwrap());
    }

    #[tokio::test]
    async fn test_save_rejects_wrong_dim() {
        let store = test_store().await;
        let err = store
            .save("evt-x", "sess-a", &[0.1, 0.2], "user_message", "h")
            .await;
        assert!(err.is_err(), "wrong-dim insert must fail");
    }

    #[tokio::test]
    async fn test_load_all() {
        let store = test_store().await;

        let emb1 = vec![0.1, 0.2, 0.3, 0.4];
        let emb2 = vec![0.5, 0.6, 0.7, 0.8];

        store
            .save("evt-1", "sess-a", &emb1, "user_message", "h1")
            .await
            .unwrap();
        store
            .save("evt-2", "sess-a", &emb2, "assistant_message", "h2")
            .await
            .unwrap();

        let all = store.load_all().await.unwrap();
        assert_eq!(all.len(), 2);

        let e1 = all.iter().find(|e| e.event_id == "evt-1").unwrap();
        assert_eq!(e1.embedding.len(), 4);
        assert!((e1.embedding[0] - 0.1f32).abs() < f32::EPSILON);
        assert!((e1.embedding[1] - 0.2f32).abs() < f32::EPSILON);

        let e2 = all.iter().find(|e| e.event_id == "evt-2").unwrap();
        assert_eq!(e2.event_type, "assistant_message");
    }

    #[tokio::test]
    async fn test_delete_by_event_ids() {
        let store = test_store().await;

        store
            .save("evt-1", "sess-a", &sample_embedding(), "user_message", "h1")
            .await
            .unwrap();
        store
            .save("evt-2", "sess-a", &sample_embedding(), "user_message", "h2")
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
            .save("evt-1", "sess-a", &sample_embedding(), "user_message", "h1")
            .await
            .unwrap();
        store
            .save("evt-2", "sess-b", &sample_embedding(), "user_message", "h2")
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
            .save("evt-1", "sess-a", &sample_embedding(), "user_message", "h1")
            .await
            .unwrap();
        store
            .save("evt-1", "sess-a", &sample_embedding(), "user_message", "h1")
            .await
            .unwrap();

        let all = store.load_all().await.unwrap();
        assert_eq!(all.len(), 1, "duplicate insert should produce only 1 row");
    }

    #[tokio::test]
    async fn test_load_all_skips_corrupted_embedding() {
        let store = test_store().await;

        let good = vec![0.1f32, 0.2f32, 0.3f32, 0.4f32];
        let bad: Vec<u8> = vec![0x01, 0x02, 0x03]; // length 3, not multiple of 4

        store
            .save("evt-good", "sess-a", &good, "user_message", "h1")
            .await
            .unwrap();

        // Directly insert corrupted blob (bypassing dim check).
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

    #[tokio::test]
    async fn test_search_similar_streaming() {
        let store = test_store().await;
        store
            .save("e1", "s", &[1.0, 0.0, 0.0, 0.0], "user_message", "h1")
            .await
            .unwrap();
        store
            .save("e2", "s", &[0.9, 0.1, 0.0, 0.0], "user_message", "h2")
            .await
            .unwrap();
        store
            .save("e3", "s", &[0.0, 0.0, 1.0, 0.0], "user_message", "h3")
            .await
            .unwrap();

        let results = store
            .search_similar(
                &[1.0, 0.0, 0.0, 0.0],
                2,
                0.5,
                &std::collections::HashSet::new(),
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "e1");
        assert!(results[0].1 >= results[1].1);
    }
}
