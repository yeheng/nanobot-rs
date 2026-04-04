//! Cross-session deduplication scanner.
//!
//! Detects potential duplicate memories by computing pairwise cosine similarity
//! within each scenario. Flags pairs above the similarity threshold for agent review.
//! Never auto-merges — only creates reports for manual resolution.

use super::embedding_store::EmbeddingStore;
use super::types::Scenario;
use anyhow::Result;
use sqlx::{Row, SqlitePool};

/// A potential duplicate pair found by the dedup scan.
#[derive(Debug, Clone)]
pub struct DedupPair {
    pub memory_a: String,
    pub memory_b: String,
    pub scenario: Scenario,
    pub similarity: f32,
    pub suggestion: DedupSuggestion,
}

/// Suggestion for how to handle a duplicate pair.
#[derive(Debug, Clone, PartialEq)]
pub enum DedupSuggestion {
    /// Memories are nearly identical — should be merged
    Merge,
    /// One memory likely supersedes the other (newer version)
    Supersede,
    /// Memories are similar but may have different context — keep both
    KeepBoth,
}

impl std::fmt::Display for DedupSuggestion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Merge => write!(f, "merge"),
            Self::Supersede => write!(f, "supersede"),
            Self::KeepBoth => write!(f, "keep-both"),
        }
    }
}

impl DedupSuggestion {
    /// Classify a similarity score into a suggestion.
    fn from_similarity(sim: f32) -> Self {
        if sim > 0.95 {
            DedupSuggestion::Merge
        } else if sim > 0.90 {
            DedupSuggestion::Supersede
        } else {
            DedupSuggestion::KeepBoth
        }
    }
}

/// Result of a dedup scan.
#[derive(Debug, Default)]
pub struct DedupReport {
    pub pairs_found: usize,
    pub pairs_inserted: usize,
    pub scenarios_scanned: usize,
    pub errors: usize,
}

/// A stored dedup report entry.
#[derive(Debug, Clone)]
pub struct DedupReportEntry {
    pub id: i64,
    pub memory_a: String,
    pub memory_b: String,
    pub similarity: f32,
    pub suggestion: String,
    pub created_at: String,
    pub resolved: bool,
}

/// Cross-session deduplication scanner.
///
/// Runs weekly (configurable) to detect potential duplicate memories within
/// each scenario using embedding similarity. Never auto-merges — only flags
/// for agent review.
pub struct DedupScanner {
    pool: SqlitePool,
    embedding_store: EmbeddingStore,
    similarity_threshold: f32,
}

impl DedupScanner {
    /// Create a new dedup scanner with default threshold (0.85).
    pub fn new(pool: SqlitePool) -> Self {
        let pool_clone = pool.clone();
        Self {
            pool,
            embedding_store: EmbeddingStore::new(pool_clone),
            similarity_threshold: 0.85,
        }
    }

    /// Create a new dedup scanner with a custom similarity threshold.
    pub fn with_threshold(pool: SqlitePool, threshold: f32) -> Self {
        let pool_clone = pool.clone();
        Self {
            pool,
            embedding_store: EmbeddingStore::new(pool_clone),
            similarity_threshold: threshold,
        }
    }

    /// Run dedup scan across all scenarios using per-file Top-K search.
    ///
    /// Instead of O(N²) pairwise comparison, for each memory embedding we use
    /// the existing `search_by_similarity` to find the Top-K nearest neighbors.
    /// This reduces complexity from O(N²) to O(N*K) where K is the neighbor limit.
    ///
    /// # Arguments
    /// * `top_k` - Number of nearest neighbors to check per file (default: 10)
    pub async fn run_scan(&self) -> Result<DedupReport> {
        self.run_scan_with_k(10).await
    }

    /// Run dedup scan with a custom Top-K neighbor limit.
    pub async fn run_scan_with_k(&self, top_k: usize) -> Result<DedupReport> {
        let mut report = DedupReport::default();

        use std::collections::HashSet;

        for scenario in Scenario::all() {
            report.scenarios_scanned += 1;

            // Get all embeddings for this scenario
            let embeddings = match self.embedding_store.get_all_for_scenario(*scenario).await {
                Ok(emb) => emb,
                Err(e) => {
                    tracing::error!("Failed to fetch embeddings for {:?}: {}", scenario, e);
                    report.errors += 1;
                    continue;
                }
            };

            // Track seen pairs to avoid duplicates (A→B and B→A)
            let mut seen_pairs: HashSet<(String, String)> = HashSet::new();
            let mut pairs: Vec<DedupPair> = Vec::new();

            for (path, vec) in &embeddings {
                // Use search_by_similarity to find Top-K nearest neighbors
                let neighbors = match self
                    .embedding_store
                    .search_by_similarity(vec, Some(*scenario), top_k)
                    .await
                {
                    Ok(n) => n,
                    Err(e) => {
                        tracing::error!("Failed to search neighbors for {}: {}", path, e);
                        report.errors += 1;
                        continue;
                    }
                };

                for hit in neighbors {
                    // Skip self-matches
                    if hit.memory_path == *path {
                        continue;
                    }

                    // Normalize pair ordering to avoid duplicates
                    let pair_key = if *path < hit.memory_path {
                        (path.clone(), hit.memory_path.clone())
                    } else {
                        (hit.memory_path.clone(), path.clone())
                    };

                    if seen_pairs.contains(&pair_key) {
                        continue;
                    }
                    seen_pairs.insert(pair_key.clone());

                    if hit.similarity > self.similarity_threshold {
                        report.pairs_found += 1;
                        pairs.push(DedupPair {
                            memory_a: pair_key.0,
                            memory_b: pair_key.1,
                            scenario: *scenario,
                            similarity: hit.similarity,
                            suggestion: DedupSuggestion::from_similarity(hit.similarity),
                        });
                    }
                }
            }

            // Insert flagged pairs into database
            for pair in pairs {
                if let Err(e) = self.insert_pair(&pair).await {
                    tracing::error!("Failed to insert dedup pair: {}", e);
                    report.errors += 1;
                } else {
                    report.pairs_inserted += 1;
                }
            }
        }

        Ok(report)
    }

    /// Insert a dedup pair into the database.
    async fn insert_pair(&self, pair: &DedupPair) -> Result<()> {
        sqlx::query(
            "INSERT INTO dedup_reports (memory_a, memory_b, similarity, suggestion) VALUES (?, ?, ?, ?)",
        )
        .bind(&pair.memory_a)
        .bind(&pair.memory_b)
        .bind(pair.similarity)
        .bind(pair.suggestion.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get unresolved dedup reports for agent review.
    ///
    /// Returns reports ordered by similarity (highest first), so the most
    /// likely duplicates are reviewed first.
    pub async fn get_pending_reports(&self) -> Result<Vec<DedupReportEntry>> {
        let rows = sqlx::query(
            "SELECT id, memory_a, memory_b, similarity, suggestion, created_at, resolved
             FROM dedup_reports
             WHERE resolved = 0
             ORDER BY similarity DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut reports = Vec::new();
        for row in rows {
            reports.push(DedupReportEntry {
                id: row.get("id"),
                memory_a: row.get("memory_a"),
                memory_b: row.get("memory_b"),
                similarity: row.get("similarity"),
                suggestion: row.get("suggestion"),
                created_at: row.get("created_at"),
                resolved: row.get::<i64, _>("resolved") != 0,
            });
        }

        Ok(reports)
    }

    /// Mark a dedup report as resolved.
    pub async fn resolve_report(&self, report_id: i64) -> Result<()> {
        sqlx::query("UPDATE dedup_reports SET resolved = 1 WHERE id = ?")
            .bind(report_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Clear all resolved reports older than N days.
    ///
    /// Returns the number of reports deleted.
    pub async fn cleanup_old_reports(&self, days: u32) -> Result<usize> {
        let result = sqlx::query(
            "DELETE FROM dedup_reports
             WHERE resolved = 1
             AND datetime(created_at) < datetime('now', '-' || ? || ' days')",
        )
        .bind(days as i64)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() as usize)
    }

    /// Calculate cosine similarity between two vectors.
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Frequency;
    use crate::SqliteStore;

    async fn setup_scanner() -> (SqliteStore, DedupScanner) {
        let temp_path =
            std::env::temp_dir().join(format!("gasket_dedup_test_{}.db", uuid::Uuid::new_v4()));
        let store = SqliteStore::with_path(temp_path).await.unwrap();
        let scanner = DedupScanner::new(store.pool().clone());
        (store, scanner)
    }

    #[tokio::test]
    async fn test_cosine_similarity() {
        // Identical vectors = 1.0
        let sim = DedupScanner::cosine_similarity(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]);
        assert!((sim - 1.0).abs() < 0.001);

        // Orthogonal vectors = 0.0
        let sim = DedupScanner::cosine_similarity(&[1.0, 0.0, 0.0], &[0.0, 1.0, 0.0]);
        assert!((sim - 0.0).abs() < 0.001);

        // Opposite vectors = -1.0
        let sim = DedupScanner::cosine_similarity(&[1.0, 1.0, 1.0], &[-1.0, -1.0, -1.0]);
        assert!((sim - (-1.0)).abs() < 0.001);

        // Empty vectors = 0.0
        let sim = DedupScanner::cosine_similarity(&[], &[1.0, 2.0]);
        assert_eq!(0.0, sim);

        // Mismatched lengths = 0.0
        let sim = DedupScanner::cosine_similarity(&[1.0, 2.0], &[1.0, 2.0, 3.0]);
        assert_eq!(0.0, sim);
    }

    #[test]
    fn test_dedup_suggestion_display() {
        assert_eq!("merge", DedupSuggestion::Merge.to_string());
        assert_eq!("supersede", DedupSuggestion::Supersede.to_string());
        assert_eq!("keep-both", DedupSuggestion::KeepBoth.to_string());
    }

    #[test]
    fn test_dedup_suggestion_from_similarity() {
        // > 0.95 → merge
        assert_eq!(
            DedupSuggestion::Merge,
            DedupSuggestion::from_similarity(0.96)
        );
        assert_eq!(
            DedupSuggestion::Merge,
            DedupSuggestion::from_similarity(0.98)
        );

        // > 0.90 → supersede
        assert_eq!(
            DedupSuggestion::Supersede,
            DedupSuggestion::from_similarity(0.91)
        );
        assert_eq!(
            DedupSuggestion::Supersede,
            DedupSuggestion::from_similarity(0.93)
        );

        // 0.85–0.90 → keep-both
        assert_eq!(
            DedupSuggestion::KeepBoth,
            DedupSuggestion::from_similarity(0.85)
        );
        assert_eq!(
            DedupSuggestion::KeepBoth,
            DedupSuggestion::from_similarity(0.87)
        );
        assert_eq!(
            DedupSuggestion::KeepBoth,
            DedupSuggestion::from_similarity(0.90)
        );

        // Below threshold (not used in practice but tested for completeness)
        assert_eq!(
            DedupSuggestion::KeepBoth,
            DedupSuggestion::from_similarity(0.80)
        );
    }

    #[tokio::test]
    async fn test_run_scan_detects_duplicates() {
        let (_store, scanner) = setup_scanner().await;

        // Insert embeddings for knowledge scenario
        scanner
            .embedding_store
            .upsert(
                "knowledge/rust.md",
                "knowledge",
                &[],
                Frequency::Warm,
                &[1.0, 0.0, 0.0],
                100,
            )
            .await
            .unwrap();

        // Insert a nearly identical embedding (should be detected)
        scanner
            .embedding_store
            .upsert(
                "knowledge/rust2.md",
                "knowledge",
                &[],
                Frequency::Warm,
                &[0.99, 0.01, 0.0],
                100,
            )
            .await
            .unwrap();

        // Run scan
        let report = scanner.run_scan().await.unwrap();

        // Should detect 1 pair
        assert_eq!(1, report.pairs_found);
        assert_eq!(1, report.pairs_inserted);
        assert_eq!(6, report.scenarios_scanned); // all 6 scenarios
        assert_eq!(0, report.errors);

        // Verify the report was inserted
        let pending = scanner.get_pending_reports().await.unwrap();
        assert_eq!(1, pending.len());
        assert!(pending[0].similarity > 0.95); // very similar
        assert_eq!("merge", pending[0].suggestion);
    }

    #[tokio::test]
    async fn test_run_scan_skips_below_threshold() {
        let (_store, scanner) = setup_scanner().await;

        // Insert orthogonal embeddings (should NOT be detected)
        scanner
            .embedding_store
            .upsert(
                "knowledge/a.md",
                "knowledge",
                &[],
                Frequency::Warm,
                &[1.0, 0.0, 0.0],
                100,
            )
            .await
            .unwrap();

        scanner
            .embedding_store
            .upsert(
                "knowledge/b.md",
                "knowledge",
                &[],
                Frequency::Warm,
                &[0.0, 1.0, 0.0],
                100,
            )
            .await
            .unwrap();

        // Run scan
        let report = scanner.run_scan().await.unwrap();

        // Should detect 0 pairs (similarity = 0.0, below threshold 0.85)
        assert_eq!(0, report.pairs_found);
        assert_eq!(0, report.pairs_inserted);
    }

    #[tokio::test]
    async fn test_get_pending_reports() {
        let (_store, scanner) = setup_scanner().await;

        // Insert a dedup report manually
        scanner
            .insert_pair(&DedupPair {
                memory_a: "knowledge/a.md".to_string(),
                memory_b: "knowledge/b.md".to_string(),
                scenario: Scenario::Knowledge,
                similarity: 0.92,
                suggestion: DedupSuggestion::Supersede,
            })
            .await
            .unwrap();

        // Insert another report and mark it resolved
        sqlx::query("INSERT INTO dedup_reports (memory_a, memory_b, similarity, suggestion, resolved) VALUES (?, ?, ?, ?, 1)")
            .bind("knowledge/c.md")
            .bind("knowledge/d.md")
            .bind(0.88)
            .bind("keep-both")
            .execute(&scanner.pool)
            .await
            .unwrap();

        // Get pending reports
        let pending = scanner.get_pending_reports().await.unwrap();

        // Should only return unresolved
        assert_eq!(1, pending.len());
        assert_eq!("knowledge/a.md", pending[0].memory_a);
        assert!(!pending[0].resolved);
    }

    #[tokio::test]
    async fn test_resolve_report() {
        let (_store, scanner) = setup_scanner().await;

        // Insert a dedup report
        scanner
            .insert_pair(&DedupPair {
                memory_a: "knowledge/a.md".to_string(),
                memory_b: "knowledge/b.md".to_string(),
                scenario: Scenario::Knowledge,
                similarity: 0.92,
                suggestion: DedupSuggestion::Supersede,
            })
            .await
            .unwrap();

        // Get the report ID
        let pending = scanner.get_pending_reports().await.unwrap();
        assert_eq!(1, pending.len());
        let report_id = pending[0].id;

        // Resolve it
        scanner.resolve_report(report_id).await.unwrap();

        // Should no longer be pending
        let pending = scanner.get_pending_reports().await.unwrap();
        assert_eq!(0, pending.len());
    }

    #[tokio::test]
    async fn test_cleanup_old_reports() {
        let (_store, scanner) = setup_scanner().await;

        // Insert an old resolved report (manually set created_at)
        sqlx::query(
            "INSERT INTO dedup_reports (memory_a, memory_b, similarity, suggestion, created_at, resolved)
             VALUES (?, ?, ?, ?, datetime('now', '-10 days'), 1)",
        )
        .bind("knowledge/old.md")
        .bind("knowledge/old2.md")
        .bind(0.90)
        .bind("keep-both")
        .execute(&scanner.pool)
        .await
        .unwrap();

        // Insert a recent resolved report (should not be deleted)
        sqlx::query(
            "INSERT INTO dedup_reports (memory_a, memory_b, similarity, suggestion, created_at, resolved)
             VALUES (?, ?, ?, ?, datetime('now', '-1 day'), 1)",
        )
        .bind("knowledge/recent.md")
        .bind("knowledge/recent2.md")
        .bind(0.90)
        .bind("keep-both")
        .execute(&scanner.pool)
        .await
        .unwrap();

        // Insert an unresolved report (should not be deleted)
        scanner
            .insert_pair(&DedupPair {
                memory_a: "knowledge/unresolved.md".to_string(),
                memory_b: "knowledge/unresolved2.md".to_string(),
                scenario: Scenario::Knowledge,
                similarity: 0.92,
                suggestion: DedupSuggestion::Supersede,
            })
            .await
            .unwrap();

        // Cleanup reports older than 7 days
        let deleted = scanner.cleanup_old_reports(7).await.unwrap();

        // Should delete only the old resolved report
        assert_eq!(1, deleted);

        // Verify only 2 reports remain (recent + unresolved)
        let count: i64 = sqlx::query("SELECT COUNT(*) FROM dedup_reports")
            .fetch_one(&scanner.pool)
            .await
            .unwrap()
            .get(0);

        assert_eq!(2, count);
    }
}
