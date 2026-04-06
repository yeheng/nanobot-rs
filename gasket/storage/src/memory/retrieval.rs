//! Retrieval engine combining tag and embedding search with frequency-boosted scoring.
//!
//! Scoring strategy (no magic numbers):
//! 1. **Tags as hard filter**: If the query has tags, results must match at least one.
//! 2. **Embedding similarity as primary score**: Raw cosine similarity (0.0–1.0).
//! 3. **Frequency as bonus multiplier**: Hot = 1.2x, Warm = 1.1x, Cold = 1.0x.
//!
//! Final score = embedding_similarity × frequency_bonus

use super::embedding_store::EmbeddingStore;
use super::metadata_store::MetadataStore;
use super::types::*;
use anyhow::Result;

/// Frequency bonus multipliers for scoring.
///
/// Hot items get a 20% boost, Warm get 10%, Cold items get no bonus.
/// Archived items are excluded from search entirely.
impl Frequency {
    pub(crate) fn bonus(self) -> f32 {
        match self {
            Frequency::Hot => 1.2,
            Frequency::Warm => 1.1,
            Frequency::Cold => 1.0,
            Frequency::Archived => 0.0, // excluded
        }
    }
}

/// A search result with combined scoring.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub memory_path: String,
    pub scenario: Scenario,
    pub title: String,
    pub tags: Vec<String>,
    pub frequency: Frequency,
    pub score: f32,
    pub tag_score: f32,
    pub embedding_score: f32,
    pub tokens: u32,
    pub id: String,
}

/// Retrieval engine combining tag and embedding search via SQLite metadata.
pub struct RetrievalEngine {
    metadata_store: MetadataStore,
    embedding_store: EmbeddingStore,
}

impl RetrievalEngine {
    pub fn new(metadata_store: MetadataStore, embedding_store: EmbeddingStore) -> Self {
        Self {
            metadata_store,
            embedding_store,
        }
    }

    /// Tag-only search via SQLite metadata.
    ///
    /// Tags act as a hard filter (must match at least one). Score is based on
    /// tag match ratio × frequency bonus.
    pub async fn search_by_tags(
        &self,
        query_tags: &[String],
        scenario: Option<Scenario>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let entries = self
            .metadata_store
            .query_by_tags(query_tags, scenario, limit)
            .await?;

        let results: Vec<SearchResult> = entries
            .into_iter()
            .filter(|e| !matches!(e.scenario, Scenario::Active | Scenario::Profile))
            .map(|entry| {
                let matching = entry
                    .tags
                    .iter()
                    .filter(|t| query_tags.iter().any(|qt| qt.eq_ignore_ascii_case(t)))
                    .count();
                let tag_score = matching as f32 / query_tags.len().max(1) as f32;
                let final_score = tag_score * entry.frequency.bonus();

                SearchResult {
                    memory_path: entry.filename,
                    scenario: entry.scenario,
                    title: entry.title,
                    tags: entry.tags,
                    frequency: entry.frequency,
                    score: final_score,
                    tag_score,
                    embedding_score: 0.0,
                    tokens: entry.tokens,
                    id: entry.id,
                }
            })
            .collect();

        Ok(results)
    }

    /// Embedding-only search: cosine similarity with frequency bonus.
    pub async fn search_by_embedding(
        &self,
        _query: &str,
        query_embedding: &[f32],
        scenario: Option<Scenario>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let hits = self
            .embedding_store
            .search_by_similarity(query_embedding, scenario, limit)
            .await?;

        let results: Vec<SearchResult> = hits
            .into_iter()
            .map(|hit| {
                let emb_score = (hit.similarity + 1.0) / 2.0; // normalize [-1,1] → [0,1]
                let scenario =
                    Scenario::from_dir_name(&hit.scenario).unwrap_or(Scenario::Knowledge);
                let frequency = Frequency::from_str_lossy(&hit.frequency);
                let final_score = emb_score * frequency.bonus();

                SearchResult {
                    memory_path: hit.memory_path,
                    scenario,
                    title: String::new(), // not stored in embedding table
                    tags: hit.tags,
                    frequency,
                    score: final_score,
                    tag_score: 0.0,
                    embedding_score: emb_score,
                    tokens: hit.token_count,
                    id: String::new(), // not stored in embedding table
                }
            })
            .collect();

        Ok(results)
    }

    /// Combined search: embedding primary score + tag hard filter + frequency bonus.
    ///
    /// Flattened strategy:
    /// 1. Get embedding results (which include tags/frequency from SQLite)
    /// 2. If tags specified, apply hard filter: score = emb × freq_bonus × tag_filter
    /// 3. If no embedding results but tags exist, fall back to tag-only search
    pub async fn search(&self, query: &MemoryQuery) -> Result<Vec<SearchResult>> {
        let limit = 20;

        // Get embedding results
        let emb_results = if query.text.is_some() {
            let dummy_emb = vec![0.0f32; 384]; // TODO: use actual embedder
            self.search_by_embedding("", &dummy_emb, query.scenario, limit)
                .await?
        } else {
            Vec::new()
        };

        // If tags specified, apply hard filter on embedding results
        if !query.tags.is_empty() {
            if !emb_results.is_empty() {
                return Ok(emb_results
                    .into_iter()
                    .filter(|r| {
                        r.tags
                            .iter()
                            .any(|t| query.tags.iter().any(|qt| qt.eq_ignore_ascii_case(t)))
                    })
                    .collect());
            }
            // No embedding results — fall back to tag-only search
            return self
                .search_by_tags(&query.tags, query.scenario, limit)
                .await;
        }

        // No tags — return embedding results as-is
        Ok(emb_results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::FileIndexManager;
    use crate::SqliteStore;
    use tempfile::TempDir;

    async fn setup_engine() -> (RetrievalEngine, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path().to_path_buf();

        let db_path = base_dir.join("test.db");
        let pool = SqliteStore::with_path(db_path)
            .await
            .unwrap()
            .pool()
            .clone();

        let index_manager = FileIndexManager::new(base_dir.join("memory"));
        let metadata_store = MetadataStore::new(pool.clone());
        let embedding_store = EmbeddingStore::new(pool);

        // Create scenario directories and sync metadata
        for scen in Scenario::all() {
            let dir = base_dir.join("memory").join(scen.dir_name());
            tokio::fs::create_dir_all(&dir).await.unwrap();
            let entries = index_manager.scan_entries(*scen).await.unwrap_or_default();
            metadata_store.sync_entries(*scen, &entries).await.unwrap();
        }

        let engine = RetrievalEngine::new(metadata_store, embedding_store);
        (engine, temp_dir)
    }

    async fn setup_engine_with_files() -> (RetrievalEngine, MetadataStore, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path().to_path_buf();

        let db_path = base_dir.join("test.db");
        let pool = SqliteStore::with_path(db_path)
            .await
            .unwrap()
            .pool()
            .clone();

        let index_manager = FileIndexManager::new(base_dir.join("memory"));
        let metadata_store = MetadataStore::new(pool.clone());
        let embedding_store = EmbeddingStore::new(pool);

        // Create scenario directories
        for scen in Scenario::all() {
            let dir = base_dir.join("memory").join(scen.dir_name());
            tokio::fs::create_dir_all(&dir).await.unwrap();
        }

        let engine = RetrievalEngine::new(metadata_store.clone(), embedding_store);
        (engine, metadata_store, temp_dir)
    }

    #[tokio::test]
    async fn test_tag_search_matches_correctly() {
        let (engine, metadata_store, temp_dir) = setup_engine_with_files().await;
        let base_dir = temp_dir.path();
        let knowledge_dir = base_dir.join("memory/knowledge");

        let memory1 = r#"---
id: mem_001
title: Rust Programming
type: note
scenario: knowledge
tags:
  - rust
  - programming
frequency: warm
created: 2026-04-03T00:00:00Z
updated: 2026-04-03T00:00:00Z
last_accessed: 2026-04-03T00:00:00Z
tokens: 100
---
"#;
        tokio::fs::write(knowledge_dir.join("rust.md"), memory1)
            .await
            .unwrap();

        let memory2 = r#"---
id: mem_002
title: Python Programming
type: note
scenario: knowledge
tags:
  - python
  - programming
frequency: warm
created: 2026-04-03T00:00:00Z
updated: 2026-04-03T00:00:00Z
last_accessed: 2026-04-03T00:00:00Z
tokens: 100
---
"#;
        tokio::fs::write(knowledge_dir.join("python.md"), memory2)
            .await
            .unwrap();

        // Sync to SQLite
        let index_manager = FileIndexManager::new(base_dir.join("memory"));
        let entries = index_manager
            .scan_entries(Scenario::Knowledge)
            .await
            .unwrap();
        metadata_store
            .sync_entries(Scenario::Knowledge, &entries)
            .await
            .unwrap();

        // Search by "rust" tag
        let results = engine
            .search_by_tags(&["rust".to_string()], Some(Scenario::Knowledge), 10)
            .await
            .unwrap();

        assert_eq!(1, results.len());
        assert_eq!(results[0].title, "Rust Programming");
        assert!(results[0].tags.contains(&"rust".to_string()));
        assert_eq!(results[0].tag_score, 1.0); // 1/1 tags matched
    }

    #[tokio::test]
    async fn test_embedding_scoring_normalizes_correctly() {
        let (_engine, metadata_store, temp_dir) = setup_engine_with_files().await;
        let base_dir = temp_dir.path();
        let knowledge_dir = base_dir.join("memory/knowledge");

        let memory1 = r#"---
id: mem_001
title: Test Memory
type: note
scenario: knowledge
tags:
  - test
  - important
frequency: warm
created: 2026-04-03T00:00:00Z
updated: 2026-04-03T00:00:00Z
last_accessed: 2026-04-03T00:00:00Z
tokens: 50
---
"#;
        tokio::fs::write(knowledge_dir.join("test.md"), memory1)
            .await
            .unwrap();

        // Add embedding
        let pool = SqliteStore::with_path(base_dir.join("test.db"))
            .await
            .unwrap()
            .pool()
            .clone();
        let emb_store = EmbeddingStore::new(pool);
        emb_store
            .upsert(
                "test.md",
                "knowledge",
                &["test".to_string(), "important".to_string()],
                Frequency::Warm,
                &[1.0, 0.0, 0.0],
                50,
            )
            .await
            .unwrap();

        let engine = RetrievalEngine::new(metadata_store.clone(), emb_store);

        // Test embedding-only search
        let emb_results = engine
            .search_by_embedding("test", &[1.0, 0.0, 0.0], Some(Scenario::Knowledge), 10)
            .await
            .unwrap();
        assert_eq!(emb_results[0].tag_score, 0.0);
        assert!((emb_results[0].embedding_score - 1.0).abs() < 0.001);
        assert!((emb_results[0].score - 1.1).abs() < 0.001); // 1.0 * 1.1 (Warm bonus)
    }

    #[tokio::test]
    async fn test_search_excludes_archived() {
        let (engine, metadata_store, temp_dir) = setup_engine_with_files().await;
        let base_dir = temp_dir.path();
        let knowledge_dir = base_dir.join("memory/knowledge");

        for (freq, title) in &[
            (Frequency::Warm, "Warm Memory"),
            (Frequency::Archived, "Archived Memory"),
        ] {
            let memory = format!(
                r#"---
id: mem_{}
title: {}
type: note
scenario: knowledge
tags:
  - test
frequency: {}
created: 2026-04-03T00:00:00Z
updated: 2026-04-03T00:00:00Z
last_accessed: 2026-04-03T00:00:00Z
tokens: 50
---
"#,
                title, title, freq
            );
            let filename = format!("{}.md", title.to_lowercase().replace(' ', "_"));
            tokio::fs::write(knowledge_dir.join(&filename), memory)
                .await
                .unwrap();
        }

        // Sync to SQLite
        let index_manager = FileIndexManager::new(base_dir.join("memory"));
        let entries = index_manager
            .scan_entries(Scenario::Knowledge)
            .await
            .unwrap();
        metadata_store
            .sync_entries(Scenario::Knowledge, &entries)
            .await
            .unwrap();

        // Search by tag — should only return warm, not archived
        let results = engine
            .search_by_tags(&["test".to_string()], Some(Scenario::Knowledge), 10)
            .await
            .unwrap();

        assert_eq!(1, results.len());
        assert_eq!(results[0].title, "Warm Memory");
        assert_eq!(results[0].frequency, Frequency::Warm);
    }

    #[tokio::test]
    async fn test_search_deduplicates_results() {
        let (_engine, metadata_store, temp_dir) = setup_engine_with_files().await;
        let base_dir = temp_dir.path();
        let knowledge_dir = base_dir.join("memory/knowledge");

        let memory1 = r#"---
id: mem_001
title: Test Memory
type: note
scenario: knowledge
tags:
  - test
frequency: warm
created: 2026-04-03T00:00:00Z
updated: 2026-04-03T00:00:00Z
last_accessed: 2026-04-03T00:00:00Z
tokens: 50
---
"#;
        tokio::fs::write(knowledge_dir.join("test.md"), memory1)
            .await
            .unwrap();

        // Sync to SQLite
        let index_manager = FileIndexManager::new(base_dir.join("memory"));
        let entries = index_manager
            .scan_entries(Scenario::Knowledge)
            .await
            .unwrap();
        metadata_store
            .sync_entries(Scenario::Knowledge, &entries)
            .await
            .unwrap();

        // Add embedding
        let pool = SqliteStore::with_path(base_dir.join("test.db"))
            .await
            .unwrap()
            .pool()
            .clone();
        let emb_store = EmbeddingStore::new(pool);
        emb_store
            .upsert(
                "test.md",
                "knowledge",
                &["test".to_string()],
                Frequency::Warm,
                &[1.0, 0.0],
                50,
            )
            .await
            .unwrap();

        let engine = RetrievalEngine::new(metadata_store, emb_store);

        // Combined search should deduplicate
        let query = MemoryQuery::new().with_tag("test").with_text("test query");
        let results = engine.search(&query).await.unwrap();

        // Should have exactly 1 result (deduplicated via embedding)
        assert_eq!(1, results.len());
        assert_eq!(results[0].memory_path, "test.md");
        assert!(results[0].embedding_score > 0.0);
        // Combined score: embedding primary × frequency bonus
        let expected = results[0].embedding_score * results[0].frequency.bonus();
        assert!((results[0].score - expected).abs() < 0.001);
    }
}
