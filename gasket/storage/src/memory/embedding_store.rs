use super::types::*;
use anyhow::Result;
use sqlx::{Row, SqlitePool};
use tracing::debug;

/// Embedding store for memory files, backed by SQLite.
pub struct EmbeddingStore {
    pool: SqlitePool,
}

/// A hit from embedding similarity search.
#[derive(Debug, Clone)]
pub struct EmbeddingHit {
    pub memory_path: String,
    pub scenario: String,
    pub tags: Vec<String>,
    pub frequency: String,
    pub similarity: f32,
    pub token_count: u32,
}

impl EmbeddingStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert or update an embedding for a memory file.
    pub async fn upsert(
        &self,
        memory_path: &str,
        scenario: &str,
        tags: &[String],
        frequency: Frequency,
        embedding: &[f32],
        token_count: u32,
    ) -> Result<()> {
        let tags_json = serde_json::to_string(tags)?;
        let embedding_bytes: &[u8] = bytemuck::cast_slice(embedding);
        let freq_str = frequency.to_string();

        sqlx::query(
            "INSERT OR REPLACE INTO memory_embeddings
             (memory_path, scenario, tags, frequency, embedding, token_count, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, datetime('now'))",
        )
        .bind(memory_path)
        .bind(scenario)
        .bind(&tags_json)
        .bind(&freq_str)
        .bind(embedding_bytes)
        .bind(token_count as i64)
        .execute(&self.pool)
        .await?;

        debug!(
            "Upserted embedding: path={}, scenario={}, freq={}, tokens={}",
            memory_path, scenario, freq_str, token_count
        );
        Ok(())
    }

    /// Delete an embedding by memory path.
    pub async fn delete(&self, memory_path: &str) -> Result<()> {
        sqlx::query("DELETE FROM memory_embeddings WHERE memory_path = ?")
            .bind(memory_path)
            .execute(&self.pool)
            .await?;
        debug!("Deleted embedding: {}", memory_path);
        Ok(())
    }

    /// Delete all embeddings — used by destructive reindex to wipe the cache.
    ///
    /// Since SQLite is a volatile cache (filesystem is SSOT), clearing all
    /// embeddings and rebuilding from disk is safe and idempotent.
    pub async fn delete_all(&self) -> Result<()> {
        sqlx::query("DELETE FROM memory_embeddings")
            .execute(&self.pool)
            .await?;
        debug!("Deleted all embeddings (reindex wipe)");
        Ok(())
    }

    /// Search by tag matching using `json_each` for accurate array-element lookup.
    ///
    /// Returns all entries whose tags JSON array contains any of the query tags.
    /// Uses `json_each` instead of fragile `LIKE` substring matching.
    pub async fn search_by_tags(
        &self,
        tags: &[String],
        scenario: Option<Scenario>,
        limit: usize,
    ) -> Result<Vec<EmbeddingHit>> {
        // Build EXISTS subqueries using json_each for each tag
        let tag_exists: Vec<String> = tags
            .iter()
            .enumerate()
            .map(|(i, _)| {
                format!(
                    "EXISTS (SELECT 1 FROM json_each(tags) WHERE json_each.value = ?{})",
                    i + 1
                )
            })
            .collect();

        let scenario_idx = tags.len() + 1;
        let limit_idx = if scenario.is_some() {
            scenario_idx + 1
        } else {
            scenario_idx
        };

        let where_scenario = if scenario.is_some() {
            format!(" AND scenario = ?{}", scenario_idx)
        } else {
            String::new()
        };

        let sql = format!(
            "SELECT memory_path, scenario, tags, frequency, token_count
             FROM memory_embeddings
             WHERE frequency != 'archived'{} AND ({})
             ORDER BY CASE frequency
                 WHEN 'hot' THEN 0
                 WHEN 'warm' THEN 1
                 WHEN 'cold' THEN 2
                 ELSE 3
             END
             LIMIT ?{}",
            where_scenario,
            tag_exists.join(" OR "),
            limit_idx
        );

        let mut query = sqlx::query(&sql);
        for tag in tags {
            query = query.bind(tag);
        }
        if let Some(s) = scenario {
            query = query.bind(s.dir_name());
        }
        query = query.bind(limit as i64);

        let rows = query.fetch_all(&self.pool).await?;
        let mut hits = Vec::new();
        for row in rows {
            let path: String = row.get("memory_path");
            let scen: String = row.get("scenario");
            let tags_str: String = row.get("tags");
            let freq: String = row.get("frequency");
            let tokens: i64 = row.get("token_count");
            let parsed_tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();
            hits.push(EmbeddingHit {
                memory_path: path,
                scenario: scen,
                tags: parsed_tags,
                frequency: freq,
                similarity: 0.0, // no embedding match
                token_count: tokens as u32,
            });
        }
        debug!(
            "Tag search: {} query tags, {} results",
            tags.len(),
            hits.len()
        );
        Ok(hits)
    }

    /// Search by embedding similarity. Fetches all non-archived vectors,
    /// computes cosine similarity in Rust, returns top-K.
    pub async fn search_by_similarity(
        &self,
        query_embedding: &[f32],
        scenario: Option<Scenario>,
        limit: usize,
    ) -> Result<Vec<EmbeddingHit>> {
        let query = if let Some(s) = scenario {
            format!(
                "SELECT memory_path, scenario, tags, frequency, embedding, token_count
                 FROM memory_embeddings
                 WHERE frequency != 'archived' AND scenario = '{}'",
                s.dir_name()
            )
        } else {
            "SELECT memory_path, scenario, tags, frequency, embedding, token_count
             FROM memory_embeddings
             WHERE frequency != 'archived'"
                .to_string()
        };

        let rows = sqlx::query(&query).fetch_all(&self.pool).await?;
        let mut scored: Vec<EmbeddingHit> = Vec::new();

        for row in rows {
            let path: String = row.get("memory_path");
            let scen: String = row.get("scenario");
            let tags_str: String = row.get("tags");
            let freq: String = row.get("frequency");
            let embedding_blob: Vec<u8> = row.get("embedding");
            let tokens: i64 = row.get("token_count");

            let stored: Vec<f32> = bytemuck::cast_slice(&embedding_blob).to_vec();
            let sim = cosine_similarity(query_embedding, &stored);

            let parsed_tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();

            scored.push(EmbeddingHit {
                memory_path: path,
                scenario: scen,
                tags: parsed_tags,
                frequency: freq,
                similarity: sim,
                token_count: tokens as u32,
            });
        }

        let candidates = scored.len();
        scored.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(limit);
        debug!(
            "Embedding search: {} candidates, {} results",
            candidates,
            scored.len()
        );
        Ok(scored)
    }

    /// Get all embeddings for a scenario (used by dedup scan).
    pub async fn get_all_for_scenario(
        &self,
        scenario: Scenario,
    ) -> Result<Vec<(String, Vec<f32>)>> {
        let query = format!(
            "SELECT memory_path, embedding FROM memory_embeddings WHERE scenario = '{}'",
            scenario.dir_name()
        );
        let rows = sqlx::query(&query).fetch_all(&self.pool).await?;
        let mut result = Vec::new();
        for row in rows {
            let path: String = row.get("memory_path");
            let blob: Vec<u8> = row.get("embedding");
            let embedding: Vec<f32> = bytemuck::cast_slice(&blob).to_vec();
            result.push((path, embedding));
        }
        Ok(result)
    }
}

/// Compute cosine similarity between two vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqliteStore;

    async fn setup_store() -> (SqliteStore, EmbeddingStore) {
        let temp_path =
            std::env::temp_dir().join(format!("gasket_emb_test_{}.db", uuid::Uuid::new_v4()));
        let store = SqliteStore::with_path(temp_path).await.unwrap();
        let emb_store = EmbeddingStore::new(store.pool().clone());
        (store, emb_store)
    }

    #[tokio::test]
    async fn test_upsert_and_read() {
        let (_store, emb) = setup_store().await;

        // Insert an embedding
        emb.upsert(
            "profile/user.md",
            "profile",
            &["important".to_string(), "user".to_string()],
            Frequency::Warm,
            &[1.0, 0.0, 0.0],
            100,
        )
        .await
        .unwrap();

        // Verify it exists via get_all
        let results = emb.get_all_for_scenario(Scenario::Profile).await.unwrap();
        assert_eq!(1, results.len());
        assert_eq!("profile/user.md", results[0].0);
        assert_eq!(&[1.0, 0.0, 0.0][..], &results[0].1[..]);
    }

    #[tokio::test]
    async fn test_search_by_similarity() {
        let (_store, emb) = setup_store().await;

        // Insert 3 embeddings with different vectors
        emb.upsert(
            "knowledge/a.md",
            "knowledge",
            &[],
            Frequency::Warm,
            &[1.0, 0.0, 0.0],
            100,
        )
        .await
        .unwrap();

        emb.upsert(
            "knowledge/b.md",
            "knowledge",
            &[],
            Frequency::Warm,
            &[0.0, 1.0, 0.0],
            100,
        )
        .await
        .unwrap();

        emb.upsert(
            "knowledge/c.md",
            "knowledge",
            &[],
            Frequency::Warm,
            &[0.9, 0.1, 0.0],
            100,
        )
        .await
        .unwrap();

        // Query with [1.0, 0.0, 0.0] - should rank a.md highest (identical), then c.md, then b.md
        let results = emb
            .search_by_similarity(&[1.0, 0.0, 0.0], Some(Scenario::Knowledge), 10)
            .await
            .unwrap();

        assert_eq!(3, results.len());
        assert_eq!("knowledge/a.md", results[0].memory_path);
        assert!((results[0].similarity - 1.0).abs() < 0.001); // identical
        assert_eq!("knowledge/c.md", results[1].memory_path);
        assert!(results[1].similarity > 0.8); // very similar
        assert_eq!("knowledge/b.md", results[2].memory_path);
        assert!(results[2].similarity < 0.1); // orthogonal
    }

    #[tokio::test]
    async fn test_search_by_tags() {
        let (_store, emb) = setup_store().await;

        // Insert memories with different tags
        emb.upsert(
            "knowledge/rust.md",
            "knowledge",
            &["rust".to_string(), "programming".to_string()],
            Frequency::Warm,
            &[1.0, 0.0],
            100,
        )
        .await
        .unwrap();

        emb.upsert(
            "knowledge/python.md",
            "knowledge",
            &["python".to_string(), "programming".to_string()],
            Frequency::Warm,
            &[0.0, 1.0],
            100,
        )
        .await
        .unwrap();

        // Search for "rust" tag
        let results = emb
            .search_by_tags(&["rust".to_string()], Some(Scenario::Knowledge), 10)
            .await
            .unwrap();

        assert_eq!(1, results.len());
        assert_eq!("knowledge/rust.md", results[0].memory_path);
        assert!(results[0].tags.contains(&"rust".to_string()));
    }

    #[tokio::test]
    async fn test_delete_removes_entry() {
        let (_store, emb) = setup_store().await;

        // Insert an embedding
        emb.upsert(
            "active/task.md",
            "active",
            &[],
            Frequency::Hot,
            &[1.0, 2.0, 3.0],
            50,
        )
        .await
        .unwrap();

        // Verify it exists
        let results = emb.get_all_for_scenario(Scenario::Active).await.unwrap();
        assert_eq!(1, results.len());

        // Delete it
        emb.delete("active/task.md").await.unwrap();

        // Verify it's gone
        let results = emb.get_all_for_scenario(Scenario::Active).await.unwrap();
        assert_eq!(0, results.len());
    }

    #[tokio::test]
    async fn test_cosine_similarity() {
        // Identical vectors = 1.0
        let sim = cosine_similarity(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]);
        assert!((sim - 1.0).abs() < 0.001);

        // Orthogonal vectors = 0.0
        let sim = cosine_similarity(&[1.0, 0.0, 0.0], &[0.0, 1.0, 0.0]);
        assert!((sim - 0.0).abs() < 0.001);

        // Opposite vectors = -1.0
        let sim = cosine_similarity(&[1.0, 1.0, 1.0], &[-1.0, -1.0, -1.0]);
        assert!((sim - (-1.0)).abs() < 0.001);

        // Empty vectors = 0.0
        let sim = cosine_similarity(&[], &[1.0, 2.0]);
        assert_eq!(0.0, sim);

        // Mismatched lengths = 0.0
        let sim = cosine_similarity(&[1.0, 2.0], &[1.0, 2.0, 3.0]);
        assert_eq!(0.0, sim);
    }
}
