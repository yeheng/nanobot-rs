//! SQLite-backed metadata store for memory files.
//!
//! All metadata queries go through this module's SQLite tables — no fragile
//! Markdown table parsing is involved. Filesystem metadata is synced into
//! SQLite at startup via `FileIndexManager::scan_entries()` and kept fresh
//! by O(1) upserts from the file watcher.

use super::index::MemoryIndexEntry;
use super::types::*;
use anyhow::Result;
use sqlx::{Row, SqlitePool};

/// SQLite-backed metadata store for memory files.
///
/// Stores metadata (id, path, scenario, frequency, tags, tokens) in a proper
/// relational table, enabling robust queries without fragile Markdown parsing.
#[derive(Clone)]
pub struct MetadataStore {
    pool: SqlitePool,
}

/// Columns selected from memory_metadata — kept as a constant to avoid
/// drift between SELECT and INSERT statements.
const META_COLUMNS: &str =
    "id, path, scenario, title, memory_type, frequency, tags, tokens, updated, last_accessed, file_mtime";

/// A decay candidate returned by `get_decay_candidates`.
#[derive(Debug, Clone)]
pub struct DecayCandidate {
    pub scenario: Scenario,
    pub filename: String,
    pub frequency: Frequency,
    pub last_accessed: String,
}

impl MetadataStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Sync filesystem scan results into SQLite for a scenario.
    ///
    /// Replaces all existing entries for the scenario with fresh scan results.
    /// Should be called at startup after `FileIndexManager::scan_entries()`.
    pub async fn sync_entries(
        &self,
        scenario: Scenario,
        entries: &[MemoryIndexEntry],
    ) -> Result<()> {
        sqlx::query("DELETE FROM memory_metadata WHERE scenario = ?")
            .bind(scenario.dir_name())
            .execute(&self.pool)
            .await?;

        for entry in entries {
            let tags_json = serde_json::to_string(&entry.tags)?;
            sqlx::query(&format!(
                "INSERT INTO memory_metadata
                 ({META_COLUMNS})
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
            ))
            .bind(&entry.id)
            .bind(&entry.filename)
            .bind(scenario.dir_name())
            .bind(&entry.title)
            .bind(&entry.memory_type)
            .bind(entry.frequency.to_string())
            .bind(&tags_json)
            .bind(entry.tokens as i64)
            .bind(&entry.updated)
            .bind(&entry.last_accessed)
            .bind(entry.file_mtime as i64)
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }

    /// Upsert a single entry.
    pub async fn upsert_entry(&self, entry: &MemoryIndexEntry) -> Result<()> {
        let tags_json = serde_json::to_string(&entry.tags)?;
        sqlx::query(&format!(
            "INSERT OR REPLACE INTO memory_metadata
             ({META_COLUMNS})
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        ))
        .bind(&entry.id)
        .bind(&entry.filename)
        .bind(entry.scenario.dir_name())
        .bind(&entry.title)
        .bind(&entry.memory_type)
        .bind(entry.frequency.to_string())
        .bind(&tags_json)
        .bind(entry.tokens as i64)
        .bind(&entry.updated)
        .bind(&entry.last_accessed)
        .bind(entry.file_mtime as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get file_mtime for a specific entry by scenario and filename.
    pub async fn get_file_mtime(&self, scenario: Scenario, filename: &str) -> Result<u64> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT file_mtime FROM memory_metadata WHERE scenario = ? AND path = ?",
        )
        .bind(scenario.dir_name())
        .bind(filename)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|(r,)| r as u64).unwrap_or(0))
    }

    /// Delete entries for a scenario.
    pub async fn delete_scenario(&self, scenario: Scenario) -> Result<()> {
        sqlx::query("DELETE FROM memory_metadata WHERE scenario = ?")
            .bind(scenario.dir_name())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete a specific entry by scenario and filename.
    pub async fn delete_by_scenario_and_path(
        &self,
        scenario: Scenario,
        filename: &str,
    ) -> Result<()> {
        sqlx::query("DELETE FROM memory_metadata WHERE scenario = ? AND path = ?")
            .bind(scenario.dir_name())
            .bind(filename)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Query entries matching tags (any-tag match), sorted by frequency priority.
    ///
    /// Archived entries are always excluded. Uses `json_each` for accurate
    /// array-element matching instead of fragile `LIKE` substring scans.
    pub async fn query_by_tags(
        &self,
        query_tags: &[String],
        scenario: Option<Scenario>,
        limit: usize,
    ) -> Result<Vec<MemoryIndexEntry>> {
        // Build EXISTS subqueries using json_each for each tag
        let tag_exists: Vec<String> = query_tags
            .iter()
            .enumerate()
            .map(|(i, _)| {
                format!(
                    "EXISTS (SELECT 1 FROM json_each(tags) WHERE json_each.value = ?{})",
                    i + 1
                )
            })
            .collect();

        let mut where_parts = vec![
            "frequency != 'archived'".to_string(),
            format!("({})", tag_exists.join(" OR ")),
        ];

        let scenario_idx = query_tags.len() + 1;
        if scenario.is_some() {
            where_parts.push(format!("scenario = ?{}", scenario_idx));
        }

        let limit_idx = if scenario.is_some() {
            scenario_idx + 1
        } else {
            scenario_idx
        };

        let sql = format!(
            "SELECT {META_COLUMNS}
             FROM memory_metadata
             WHERE {}
             ORDER BY CASE frequency
                 WHEN 'hot' THEN 0 WHEN 'warm' THEN 1 WHEN 'cold' THEN 2 ELSE 3
             END
             LIMIT ?{}",
            where_parts.join(" AND "),
            limit_idx
        );

        let mut q = sqlx::query(&sql);
        for tag in query_tags {
            q = q.bind(tag);
        }
        if let Some(s) = scenario {
            q = q.bind(s.dir_name());
        }
        q = q.bind(limit as i64);

        let rows = q.fetch_all(&self.pool).await?;
        Ok(Self::parse_rows(rows))
    }

    /// Query all non-archived entries for a scenario, sorted by frequency.
    pub async fn query_entries(&self, scenario: Scenario) -> Result<Vec<MemoryIndexEntry>> {
        let sql = format!(
            "SELECT {META_COLUMNS}
             FROM memory_metadata
             WHERE scenario = '{}' AND frequency != 'archived'
             ORDER BY CASE frequency
                 WHEN 'hot' THEN 0 WHEN 'warm' THEN 1 WHEN 'cold' THEN 2 ELSE 3
             END",
            scenario.dir_name()
        );

        self.rows_to_entries(&sql).await
    }

    /// Query entries for context loading, ordered by priority.
    ///
    /// Returns: profile → active(hot, warm) → scenario(hot, warm).
    /// Excludes cold and archived. Warm scenario items are filtered by tag
    /// match when tags are provided — using SQL `json_each` EXISTS subqueries
    /// so SQLite returns only matching rows instead of loading the full table.
    pub async fn query_for_loading(
        &self,
        scenario: Scenario,
        tags: &[String],
    ) -> Result<Vec<MemoryIndexEntry>> {
        if tags.is_empty() {
            // No tag filtering — simple query, return all warm items
            let sql = format!(
                "SELECT {META_COLUMNS}
                 FROM memory_metadata
                 WHERE frequency NOT IN ('archived', 'cold')
                   AND (scenario IN ('profile', 'active') OR scenario = '{}')
                 ORDER BY
                   CASE scenario WHEN 'profile' THEN 0 WHEN 'active' THEN 1 ELSE 2 END,
                   CASE frequency WHEN 'hot' THEN 0 WHEN 'warm' THEN 1 ELSE 2 END",
                scenario.dir_name()
            );
            return self.rows_to_entries(&sql).await;
        }

        // Build tag EXISTS subqueries using json_each for accurate matching
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

        // profile/active + hot items always included; warm scenario items require tag match
        let sql = format!(
            "SELECT {META_COLUMNS}
             FROM memory_metadata
             WHERE frequency NOT IN ('archived', 'cold')
               AND (scenario IN ('profile', 'active')
                    OR frequency = 'hot'
                    OR (scenario = '{}' AND ({})))
             ORDER BY
               CASE scenario WHEN 'profile' THEN 0 WHEN 'active' THEN 1 ELSE 2 END,
               CASE frequency WHEN 'hot' THEN 0 WHEN 'warm' THEN 1 ELSE 2 END",
            scenario.dir_name(),
            tag_exists.join(" OR ")
        );

        let mut q = sqlx::query(&sql);
        for tag in tags {
            q = q.bind(tag);
        }

        let rows = q.fetch_all(&self.pool).await?;
        Ok(Self::parse_rows(rows))
    }

    /// Get decay candidates: entries whose `last_accessed` is older than the
    /// given threshold **and** whose frequency has not yet hit bottom (archived).
    ///
    /// This is the SQL-driven replacement for O(N) filesystem scanning in
    /// `run_decay_batch`. Only entries that *might* need decay are returned,
    /// so the caller reads at most O(k) files instead of O(N).
    ///
    /// Exempt scenarios (profile, decisions, reference) are excluded since
    /// they never decay.
    pub async fn get_decay_candidates(&self, older_than_days: i64) -> Result<Vec<DecayCandidate>> {
        let sql = format!(
            "SELECT path, scenario, frequency, last_accessed
             FROM memory_metadata
             WHERE frequency != 'archived'
               AND scenario NOT IN ('profile', 'decisions', 'reference')
               AND last_accessed != ''
               AND datetime(last_accessed) < datetime('now', '-{} days')",
            older_than_days
        );

        let rows = sqlx::query(&sql).fetch_all(&self.pool).await?;

        let candidates = rows
            .into_iter()
            .map(|row| {
                let scen_str: String = row.get("scenario");
                let freq_str: String = row.get("frequency");
                DecayCandidate {
                    scenario: Scenario::from_dir_name(&scen_str).unwrap_or(Scenario::Knowledge),
                    filename: row.get("path"),
                    frequency: Frequency::from_str_lossy(&freq_str),
                    last_accessed: row.get("last_accessed"),
                }
            })
            .collect();

        Ok(candidates)
    }

    /// Execute raw SQL and parse rows into MemoryIndexEntry.
    async fn rows_to_entries(&self, sql: &str) -> Result<Vec<MemoryIndexEntry>> {
        let rows = sqlx::query(sql).fetch_all(&self.pool).await?;
        Ok(Self::parse_rows(rows))
    }

    /// Parse SQLite rows into MemoryIndexEntry structs.
    fn parse_rows(rows: Vec<sqlx::sqlite::SqliteRow>) -> Vec<MemoryIndexEntry> {
        rows.into_iter()
            .map(|row| {
                let tags_str: String = row.get("tags");
                let tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();
                let freq_str: String = row.get("frequency");
                let scen_str: String = row.get("scenario");
                let last_accessed: String = row.try_get("last_accessed").unwrap_or_default();
                let file_mtime: i64 = row.try_get("file_mtime").unwrap_or(0);

                MemoryIndexEntry {
                    id: row.get("id"),
                    title: row.get("title"),
                    memory_type: row.get("memory_type"),
                    tags,
                    frequency: Frequency::from_str_lossy(&freq_str),
                    tokens: row.get::<i64, _>("tokens") as u32,
                    filename: row.get("path"),
                    updated: row.get("updated"),
                    scenario: Scenario::from_dir_name(&scen_str).unwrap_or(Scenario::Knowledge),
                    last_accessed,
                    file_mtime: file_mtime as u64,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqliteStore;

    async fn setup() -> (SqliteStore, MetadataStore) {
        let path =
            std::env::temp_dir().join(format!("gasket_metadata_test_{}.db", uuid::Uuid::new_v4()));
        let store = SqliteStore::with_path(path).await.unwrap();
        let meta = MetadataStore::new(store.pool().clone());
        (store, meta)
    }

    fn make_entry(
        id: &str,
        title: &str,
        tags: Vec<&str>,
        freq: Frequency,
        scenario: Scenario,
        last_accessed: &str,
    ) -> MemoryIndexEntry {
        MemoryIndexEntry {
            id: id.into(),
            title: title.into(),
            memory_type: "note".into(),
            tags: tags.into_iter().map(|s| s.to_string()).collect(),
            frequency: freq,
            tokens: 100,
            filename: format!("{}.md", id),
            updated: "2026-04-06".into(),
            scenario,
            last_accessed: last_accessed.into(),
            file_mtime: 0, // Test doesn't need real mtime
        }
    }

    #[tokio::test]
    async fn test_sync_and_query() {
        let (_store, meta) = setup().await;

        let entries = vec![make_entry(
            "mem_001",
            "Test",
            vec!["rust"],
            Frequency::Hot,
            Scenario::Knowledge,
            "2026-04-06T00:00:00Z",
        )];

        meta.sync_entries(Scenario::Knowledge, &entries)
            .await
            .unwrap();

        let result = meta.query_entries(Scenario::Knowledge).await.unwrap();
        assert_eq!(1, result.len());
        assert_eq!("Test", result[0].title);
        assert_eq!(Frequency::Hot, result[0].frequency);
        assert_eq!(Scenario::Knowledge, result[0].scenario);
    }

    #[tokio::test]
    async fn test_query_by_tags() {
        let (_store, meta) = setup().await;

        let entries = vec![
            make_entry(
                "mem_001",
                "Rust Note",
                vec!["rust", "programming"],
                Frequency::Warm,
                Scenario::Knowledge,
                "2026-04-06T00:00:00Z",
            ),
            make_entry(
                "mem_002",
                "Python Note",
                vec!["python"],
                Frequency::Warm,
                Scenario::Knowledge,
                "2026-04-06T00:00:00Z",
            ),
        ];

        meta.sync_entries(Scenario::Knowledge, &entries)
            .await
            .unwrap();

        let result = meta
            .query_by_tags(&["rust".into()], Some(Scenario::Knowledge), 10)
            .await
            .unwrap();

        assert_eq!(1, result.len());
        assert_eq!("Rust Note", result[0].title);
    }

    #[tokio::test]
    async fn test_query_by_tags_disambiguates_substrings() {
        let (_store, meta) = setup().await;

        let entries = vec![
            make_entry(
                "mem_001",
                "Rust Note",
                vec!["rust"],
                Frequency::Warm,
                Scenario::Knowledge,
                "2026-04-06T00:00:00Z",
            ),
            make_entry(
                "mem_002",
                "Rustlang Note",
                vec!["rustlang"],
                Frequency::Warm,
                Scenario::Knowledge,
                "2026-04-06T00:00:00Z",
            ),
        ];

        meta.sync_entries(Scenario::Knowledge, &entries)
            .await
            .unwrap();

        // Search for "rust" should NOT match "rustlang"
        let result = meta
            .query_by_tags(&["rust".into()], Some(Scenario::Knowledge), 10)
            .await
            .unwrap();

        assert_eq!(1, result.len());
        assert_eq!("Rust Note", result[0].title);

        // Search for "rustlang" should NOT match "rust"
        let result = meta
            .query_by_tags(&["rustlang".into()], Some(Scenario::Knowledge), 10)
            .await
            .unwrap();

        assert_eq!(1, result.len());
        assert_eq!("Rustlang Note", result[0].title);
    }

    #[tokio::test]
    async fn test_query_for_loading_orders_by_priority() {
        let (_store, meta) = setup().await;

        // Profile entry
        let profile = vec![make_entry(
            "mem_profile",
            "User Profile",
            vec![],
            Frequency::Hot,
            Scenario::Profile,
            "2026-04-06T00:00:00Z",
        )];
        meta.sync_entries(Scenario::Profile, &profile)
            .await
            .unwrap();

        // Active entry
        let active = vec![make_entry(
            "mem_active",
            "Active Task",
            vec![],
            Frequency::Hot,
            Scenario::Active,
            "2026-04-06T00:00:00Z",
        )];
        meta.sync_entries(Scenario::Active, &active).await.unwrap();

        // Knowledge entry
        let knowledge = vec![make_entry(
            "mem_know",
            "Knowledge",
            vec!["test"],
            Frequency::Warm,
            Scenario::Knowledge,
            "2026-04-06T00:00:00Z",
        )];
        meta.sync_entries(Scenario::Knowledge, &knowledge)
            .await
            .unwrap();

        let result = meta
            .query_for_loading(Scenario::Knowledge, &[])
            .await
            .unwrap();

        assert_eq!(3, result.len());
        // Profile first, then active, then knowledge
        assert_eq!(Scenario::Profile, result[0].scenario);
        assert_eq!(Scenario::Active, result[1].scenario);
        assert_eq!(Scenario::Knowledge, result[2].scenario);
    }

    #[tokio::test]
    async fn test_sync_replaces_existing() {
        let (_store, meta) = setup().await;

        let v1 = vec![make_entry(
            "mem_001",
            "V1",
            vec![],
            Frequency::Warm,
            Scenario::Knowledge,
            "2026-04-06T00:00:00Z",
        )];
        meta.sync_entries(Scenario::Knowledge, &v1).await.unwrap();

        let v2 = vec![make_entry(
            "mem_002",
            "V2",
            vec![],
            Frequency::Hot,
            Scenario::Knowledge,
            "2026-04-06T00:00:00Z",
        )];
        meta.sync_entries(Scenario::Knowledge, &v2).await.unwrap();

        let result = meta.query_entries(Scenario::Knowledge).await.unwrap();
        assert_eq!(1, result.len());
        assert_eq!("V2", result[0].title);
    }

    #[tokio::test]
    async fn test_query_for_loading_filters_by_tags_in_sql() {
        let (_store, meta) = setup().await;

        // Knowledge warm with matching tag
        let matching = make_entry(
            "mem_match",
            "Matching",
            vec!["rust"],
            Frequency::Warm,
            Scenario::Knowledge,
            "2026-04-06T00:00:00Z",
        );
        // Knowledge warm with non-matching tag
        let non_matching = make_entry(
            "mem_nomatch",
            "No Match",
            vec!["python"],
            Frequency::Warm,
            Scenario::Knowledge,
            "2026-04-06T00:00:00Z",
        );
        // Knowledge hot — should always appear regardless of tags
        let hot = make_entry(
            "mem_hot",
            "Hot Item",
            vec!["python"],
            Frequency::Hot,
            Scenario::Knowledge,
            "2026-04-06T00:00:00Z",
        );

        meta.sync_entries(Scenario::Knowledge, &[matching, non_matching, hot])
            .await
            .unwrap();

        // Query with tag "rust"
        let result = meta
            .query_for_loading(Scenario::Knowledge, &["rust".to_string()])
            .await
            .unwrap();

        // Should get: matching (warm, tag match) + hot (always included)
        // Should NOT get: non_matching (warm, tag mismatch)
        assert_eq!(2, result.len());
        assert!(result.iter().any(|e| e.id == "mem_match"));
        assert!(result.iter().any(|e| e.id == "mem_hot"));
        assert!(!result.iter().any(|e| e.id == "mem_nomatch"));
    }

    #[tokio::test]
    async fn test_get_decay_candidates_returns_only_stale() {
        let (_store, meta) = setup().await;

        // Stale hot entry (8 days old) — should be a candidate
        let stale = make_entry(
            "mem_stale",
            "Stale",
            vec![],
            Frequency::Hot,
            Scenario::Knowledge,
            // 8 days ago
            &(chrono::Utc::now() - chrono::Duration::days(8)).to_rfc3339(),
        );
        // Fresh entry — should NOT be a candidate
        let fresh = make_entry(
            "mem_fresh",
            "Fresh",
            vec![],
            Frequency::Hot,
            Scenario::Knowledge,
            // 1 day ago
            &(chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339(),
        );
        // Profile entry — exempt, never decays
        let profile = make_entry(
            "mem_profile",
            "Profile",
            vec![],
            Frequency::Hot,
            Scenario::Profile,
            // 100 days ago, but exempt
            &(chrono::Utc::now() - chrono::Duration::days(100)).to_rfc3339(),
        );

        meta.sync_entries(Scenario::Knowledge, &[stale, fresh])
            .await
            .unwrap();
        meta.sync_entries(Scenario::Profile, &[profile])
            .await
            .unwrap();

        // Look for entries not accessed in 7+ days
        let candidates = meta.get_decay_candidates(7).await.unwrap();

        assert_eq!(1, candidates.len());
        assert_eq!("mem_stale.md", candidates[0].filename);
        assert_eq!(Frequency::Hot, candidates[0].frequency);
        assert_eq!(Scenario::Knowledge, candidates[0].scenario);
    }
}
