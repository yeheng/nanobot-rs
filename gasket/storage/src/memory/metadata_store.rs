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
            sqlx::query(
                "INSERT INTO memory_metadata
                 (id, path, scenario, title, memory_type, frequency, tags, tokens, updated)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&entry.id)
            .bind(&entry.filename)
            .bind(scenario.dir_name())
            .bind(&entry.title)
            .bind(&entry.memory_type)
            .bind(entry.frequency.to_string())
            .bind(&tags_json)
            .bind(entry.tokens as i64)
            .bind(&entry.updated)
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }

    /// Upsert a single entry.
    pub async fn upsert_entry(&self, entry: &MemoryIndexEntry) -> Result<()> {
        let tags_json = serde_json::to_string(&entry.tags)?;
        sqlx::query(
            "INSERT OR REPLACE INTO memory_metadata
             (id, path, scenario, title, memory_type, frequency, tags, tokens, updated)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&entry.id)
        .bind(&entry.filename)
        .bind(entry.scenario.dir_name())
        .bind(&entry.title)
        .bind(&entry.memory_type)
        .bind(entry.frequency.to_string())
        .bind(&tags_json)
        .bind(entry.tokens as i64)
        .bind(&entry.updated)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Delete entries for a scenario.
    pub async fn delete_scenario(&self, scenario: Scenario) -> Result<()> {
        sqlx::query("DELETE FROM memory_metadata WHERE scenario = ?")
            .bind(scenario.dir_name())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Query entries matching tags (any-tag match), sorted by frequency priority.
    ///
    /// Archived entries are always excluded.
    pub async fn query_by_tags(
        &self,
        query_tags: &[String],
        scenario: Option<Scenario>,
        limit: usize,
    ) -> Result<Vec<MemoryIndexEntry>> {
        let tag_conds: Vec<String> = query_tags
            .iter()
            .map(|t| format!("tags LIKE '%\"{}\"%'", t.replace('\'', "''")))
            .collect();

        let mut where_parts = vec![
            "frequency != 'archived'".to_string(),
            format!("({})", tag_conds.join(" OR ")),
        ];

        if let Some(s) = scenario {
            where_parts.push(format!("scenario = '{}'", s.dir_name()));
        }

        let sql = format!(
            "SELECT id, path, scenario, title, memory_type, frequency, tags, tokens, updated
             FROM memory_metadata
             WHERE {}
             ORDER BY CASE frequency
                 WHEN 'hot' THEN 0 WHEN 'warm' THEN 1 WHEN 'cold' THEN 2 ELSE 3
             END
             LIMIT {}",
            where_parts.join(" AND "),
            limit
        );

        self.rows_to_entries(&sql).await
    }

    /// Query all non-archived entries for a scenario, sorted by frequency.
    pub async fn query_entries(&self, scenario: Scenario) -> Result<Vec<MemoryIndexEntry>> {
        let sql = format!(
            "SELECT id, path, scenario, title, memory_type, frequency, tags, tokens, updated
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
    /// match when tags are provided.
    pub async fn query_for_loading(
        &self,
        scenario: Scenario,
        tags: &[String],
    ) -> Result<Vec<MemoryIndexEntry>> {
        let sql = format!(
            "SELECT id, path, scenario, title, memory_type, frequency, tags, tokens, updated
             FROM memory_metadata
             WHERE frequency NOT IN ('archived', 'cold')
               AND (scenario IN ('profile', 'active') OR scenario = '{}')
             ORDER BY
               CASE scenario WHEN 'profile' THEN 0 WHEN 'active' THEN 1 ELSE 2 END,
               CASE frequency WHEN 'hot' THEN 0 WHEN 'warm' THEN 1 ELSE 2 END",
            scenario.dir_name()
        );

        let mut entries = self.rows_to_entries(&sql).await?;

        // Filter warm items for the target scenario by tag match
        if !tags.is_empty() {
            entries.retain(|e| {
                matches!(e.scenario, Scenario::Profile | Scenario::Active)
                    || e.frequency == Frequency::Hot
                    || e.tags
                        .iter()
                        .any(|t| tags.iter().any(|qt| qt.eq_ignore_ascii_case(t)))
            });
        }

        Ok(entries)
    }

    /// Execute SQL and parse rows into MemoryIndexEntry.
    async fn rows_to_entries(&self, sql: &str) -> Result<Vec<MemoryIndexEntry>> {
        let rows = sqlx::query(sql).fetch_all(&self.pool).await?;

        let entries = rows
            .into_iter()
            .map(|row| {
                let tags_str: String = row.get("tags");
                let tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();
                let freq_str: String = row.get("frequency");
                let scen_str: String = row.get("scenario");

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
                }
            })
            .collect();

        Ok(entries)
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

    #[tokio::test]
    async fn test_sync_and_query() {
        let (_store, meta) = setup().await;

        let entries = vec![MemoryIndexEntry {
            id: "mem_001".into(),
            title: "Test".into(),
            memory_type: "note".into(),
            tags: vec!["rust".into()],
            frequency: Frequency::Hot,
            tokens: 100,
            filename: "test.md".into(),
            updated: "2026-04-06".into(),
            scenario: Scenario::Knowledge,
        }];

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
            MemoryIndexEntry {
                id: "mem_001".into(),
                title: "Rust Note".into(),
                memory_type: "note".into(),
                tags: vec!["rust".into(), "programming".into()],
                frequency: Frequency::Warm,
                tokens: 100,
                filename: "rust.md".into(),
                updated: "2026-04-06".into(),
                scenario: Scenario::Knowledge,
            },
            MemoryIndexEntry {
                id: "mem_002".into(),
                title: "Python Note".into(),
                memory_type: "note".into(),
                tags: vec!["python".into()],
                frequency: Frequency::Warm,
                tokens: 80,
                filename: "python.md".into(),
                updated: "2026-04-06".into(),
                scenario: Scenario::Knowledge,
            },
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
    async fn test_query_for_loading_orders_by_priority() {
        let (_store, meta) = setup().await;

        // Profile entry
        let profile = vec![MemoryIndexEntry {
            id: "mem_profile".into(),
            title: "User Profile".into(),
            memory_type: "profile".into(),
            tags: vec![],
            frequency: Frequency::Hot,
            tokens: 100,
            filename: "user.md".into(),
            updated: "2026-04-06".into(),
            scenario: Scenario::Profile,
        }];
        meta.sync_entries(Scenario::Profile, &profile)
            .await
            .unwrap();

        // Active entry
        let active = vec![MemoryIndexEntry {
            id: "mem_active".into(),
            title: "Active Task".into(),
            memory_type: "task".into(),
            tags: vec![],
            frequency: Frequency::Hot,
            tokens: 200,
            filename: "task.md".into(),
            updated: "2026-04-06".into(),
            scenario: Scenario::Active,
        }];
        meta.sync_entries(Scenario::Active, &active).await.unwrap();

        // Knowledge entry
        let knowledge = vec![MemoryIndexEntry {
            id: "mem_know".into(),
            title: "Knowledge".into(),
            memory_type: "note".into(),
            tags: vec!["test".into()],
            frequency: Frequency::Warm,
            tokens: 150,
            filename: "know.md".into(),
            updated: "2026-04-06".into(),
            scenario: Scenario::Knowledge,
        }];
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

        let v1 = vec![MemoryIndexEntry {
            id: "mem_001".into(),
            title: "V1".into(),
            memory_type: "note".into(),
            tags: vec![],
            frequency: Frequency::Warm,
            tokens: 100,
            filename: "test.md".into(),
            updated: "2026-04-06".into(),
            scenario: Scenario::Knowledge,
        }];
        meta.sync_entries(Scenario::Knowledge, &v1).await.unwrap();

        let v2 = vec![MemoryIndexEntry {
            id: "mem_002".into(),
            title: "V2".into(),
            memory_type: "note".into(),
            tags: vec![],
            frequency: Frequency::Hot,
            tokens: 50,
            filename: "test2.md".into(),
            updated: "2026-04-06".into(),
            scenario: Scenario::Knowledge,
        }];
        meta.sync_entries(Scenario::Knowledge, &v2).await.unwrap();

        let result = meta.query_entries(Scenario::Knowledge).await.unwrap();
        assert_eq!(1, result.len());
        assert_eq!("V2", result[0].title);
    }
}
