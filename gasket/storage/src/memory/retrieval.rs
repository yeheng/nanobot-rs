//! Retrieval engine combining tag and embedding search with explicit ordering.
//!
//! Strategy:
//! 1. **Tags as hard filter**: If the query has tags, results must match at least one.
//! 2. **Embedding similarity as primary score**: Raw cosine similarity (0.0–1.0).
//! 3. **Frequency as explicit sort key**: Hot > Warm > Cold.
//!
//! Results are sorted by `(frequency desc, score desc)` using tuple comparison.
//! This eliminates magic-number multipliers and NaN-prone float comparisons.

use super::embedder::Embedder;
use super::embedding_store::EmbeddingStore;
use super::index::MemoryIndexEntry;
use super::metadata_store::MetadataStore;
use super::types::*;
use anyhow::Result;
use std::cmp::Ordering;

/// A search result with combined scoring.
///
/// Sort order (via `Ord`):
/// 1. Exempt scenarios (Profile, Decisions, Reference) always first
/// 2. Skill-type memories before note-type (procedural knowledge is actionable)
/// 3. Higher frequency (Hot > Warm > Cold)
/// 4. Higher score (when available)
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub memory_path: String,
    pub scenario: Scenario,
    pub title: String,
    pub tags: Vec<String>,
    pub frequency: Frequency,
    pub score: f32,
    pub tokens: u32,
    pub id: String,
    pub memory_type: String,
}

impl PartialEq for SearchResult {
    fn eq(&self, other: &Self) -> bool {
        self.scenario == other.scenario && self.memory_path == other.memory_path
    }
}

impl Eq for SearchResult {}

impl Ord for SearchResult {
    fn cmp(&self, other: &Self) -> Ordering {
        let self_exempt = self.scenario.is_exempt_from_decay();
        let other_exempt = other.scenario.is_exempt_from_decay();
        let self_skill = self.memory_type == "skill";
        let other_skill = other.memory_type == "skill";

        // 1. Exempt scenarios first
        other_exempt
            .cmp(&self_exempt)
            // 2. Skill-type memories before note-type (procedural knowledge is actionable)
            .then_with(|| other_skill.cmp(&self_skill))
            // 3. Higher frequency first
            .then_with(|| other.frequency.cmp(&self.frequency))
            // 4. Higher score first
            .then_with(|| {
                other
                    .score
                    .partial_cmp(&self.score)
                    .unwrap_or(Ordering::Equal)
            })
    }
}

impl PartialOrd for SearchResult {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl From<MemoryIndexEntry> for SearchResult {
    fn from(entry: MemoryIndexEntry) -> Self {
        Self {
            memory_path: entry.filename,
            scenario: entry.scenario,
            title: entry.title,
            tags: entry.tags,
            frequency: entry.frequency,
            score: 0.0,
            tokens: entry.tokens,
            id: entry.id,
            memory_type: entry.memory_type,
        }
    }
}

/// Retrieval engine combining tag and embedding search via SQLite metadata.
///
/// Holds an `Embedder` reference for computing real query embeddings.
/// Falls back to tag-only search when the embedder fails or produces
/// degenerate (all-zero) vectors.
pub struct RetrievalEngine {
    metadata_store: MetadataStore,
    embedding_store: EmbeddingStore,
    embedder: Option<Box<dyn Embedder>>,
}

impl RetrievalEngine {
    pub fn new(
        metadata_store: MetadataStore,
        embedding_store: EmbeddingStore,
        embedder: Option<Box<dyn Embedder>>,
    ) -> Self {
        Self {
            metadata_store,
            embedding_store,
            embedder,
        }
    }

    /// Tag-only search via SQLite metadata.
    ///
    /// Tags act as a hard filter (must match at least one). Score is based on
    /// tag match ratio.
    ///
    /// **Note:** Excludes Active and Profile scenarios — the bootstrap phase
    /// in `MemoryManager` loads these unconditionally, so tag search skips them
    /// to avoid duplication in combined search results.
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

        let mut results: Vec<SearchResult> = entries
            .into_iter()
            .filter(|e| !matches!(e.scenario, Scenario::Active | Scenario::Profile))
            .map(|entry| {
                let matching = entry
                    .tags
                    .iter()
                    .filter(|t| query_tags.iter().any(|qt| qt.eq_ignore_ascii_case(t)))
                    .count();
                let tag_score = matching as f32 / query_tags.len().max(1) as f32;

                SearchResult {
                    memory_path: entry.filename,
                    scenario: entry.scenario,
                    title: entry.title,
                    tags: entry.tags,
                    frequency: entry.frequency,
                    score: tag_score,
                    tokens: entry.tokens,
                    id: entry.id,
                    memory_type: entry.memory_type,
                }
            })
            .collect();

        // Sort using unified Ord impl (exempt > frequency > score)
        results.sort();

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

        let mut results: Vec<SearchResult> = hits
            .into_iter()
            .map(|hit| {
                let emb_score = (hit.similarity + 1.0) / 2.0; // normalize [-1,1] → [0,1]
                let scenario =
                    Scenario::from_dir_name(&hit.scenario).unwrap_or(Scenario::Knowledge);
                let frequency = Frequency::from_str_lossy(&hit.frequency);

                SearchResult {
                    memory_path: hit.memory_path,
                    scenario,
                    title: String::new(), // not stored in embedding table
                    tags: hit.tags,
                    frequency,
                    score: emb_score,
                    tokens: hit.token_count,
                    id: String::new(), // not stored in embedding table
                    memory_type: String::new(), // not stored in embedding table; will sort as non-skill
                }
            })
            .collect();

        // Sort using unified Ord impl (exempt > frequency > score)
        results.sort();

        Ok(results)
    }

    /// Compute query embedding via the embedder, if available.
    ///
    /// Returns `None` when: no text, no embedder, embedding fails,
    /// or the result is a degenerate all-zero vector (NoopEmbedder fallback).
    async fn try_embed(&self, text: &str) -> Option<Vec<f32>> {
        let embedder = self.embedder.as_ref()?;
        let vec = embedder.embed(text).await.ok()?;
        if vec.iter().all(|&v| v == 0.0) {
            return None;
        }
        Some(vec)
    }

    /// Combined search: embedding primary score + tag hard filter + frequency bonus.
    ///
    /// 1. Compute real query embedding via the embedder (if available and text provided)
    /// 2. Search by embedding similarity
    /// 3. If tags specified, apply hard filter on embedding results
    /// 4. Fall back to tag-only search if embedding fails or no text provided
    pub async fn search(&self, query: &MemoryQuery) -> Result<Vec<SearchResult>> {
        let limit = 20;

        // Compute real query embedding (returns None when no text / no embedder / failure)
        let emb_results = match &query.text {
            Some(text) => match self.try_embed(text).await {
                Some(query_emb) => self
                    .search_by_embedding(text, &query_emb, query.scenario, limit)
                    .await
                    .unwrap_or_default(),
                None => Vec::new(),
            },
            None => Vec::new(),
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

        // No tags — return embedding results as-is, or fall back to tag search
        if emb_results.is_empty() && query.text.is_some() {
            return Ok(Vec::new());
        }

        Ok(emb_results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::FileIndexManager;
    use crate::SqliteStore;
    use tempfile::TempDir;

    async fn setup_engine_with_files() -> (RetrievalEngine, MetadataStore, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path().to_path_buf();

        let db_path = base_dir.join("test.db");
        let pool = SqliteStore::with_path(db_path)
            .await
            .unwrap()
            .pool()
            .clone();

        let metadata_store = MetadataStore::new(pool.clone());
        let embedding_store = EmbeddingStore::new(pool);

        // Create scenario directories
        for scen in Scenario::all() {
            let dir = base_dir.join("memory").join(scen.dir_name());
            tokio::fs::create_dir_all(&dir).await.unwrap();
        }

        let engine = RetrievalEngine::new(metadata_store.clone(), embedding_store, None);
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
        assert_eq!(results[0].score, 1.0); // 1/1 tags matched
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

        let engine = RetrievalEngine::new(metadata_store.clone(), emb_store, None);

        // Test embedding-only search
        let emb_results = engine
            .search_by_embedding("test", &[1.0, 0.0, 0.0], Some(Scenario::Knowledge), 10)
            .await
            .unwrap();
        assert!((emb_results[0].score - 1.0).abs() < 0.001); // raw score, no frequency multiplier
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

        let engine = RetrievalEngine::new(metadata_store, emb_store, None);

        // Combined search should deduplicate
        let query = MemoryQuery::new().with_tag("test").with_text("test query");
        let results = engine.search(&query).await.unwrap();

        // Should have exactly 1 result (deduplicated)
        assert_eq!(1, results.len());
        assert_eq!(results[0].memory_path, "test.md");
        // Without a real embedder, falls back to tag search (score > 0)
        assert!(results[0].score > 0.0);
    }
}
