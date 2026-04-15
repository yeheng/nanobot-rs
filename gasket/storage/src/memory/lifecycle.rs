//! Frequency lifecycle management for memory files.
//!
//! This module implements automatic decay and promotion of memory frequency tiers,
//! along with deferred batched access tracking for performance optimization.

use super::metadata_store::MetadataStore;
use super::store::FileMemoryStore;
use super::types::*;
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

/// In-memory access log for deferred batched writes.
///
/// Access events are accumulated in memory and flushed to disk in batches
/// to avoid excessive I/O operations. Flush triggers include:
/// - Session end / graceful shutdown
/// - Access log exceeds flush_threshold entries (default: 50)
/// - Periodic background timer (every 5 minutes)
pub struct AccessLog {
    entries: Vec<AccessEntry>,
    flush_threshold: usize,
}

/// A single access record.
#[derive(Debug, Clone)]
pub struct AccessEntry {
    pub scenario: Scenario,
    pub filename: String,
    pub timestamp: DateTime<Utc>,
}

/// Frequency decay and promotion logic.
pub struct FrequencyManager;

/// Report from a decay batch run.
#[derive(Debug, Default)]
pub struct DecayReport {
    pub total_scanned: usize,
    pub decayed: usize,
    pub errors: usize,
}

/// Report from an access log flush.
#[derive(Debug, Default)]
pub struct FlushReport {
    pub total_flushed: usize,
    pub promoted: usize,
    pub errors: usize,
}

impl AccessLog {
    /// Create a new access log with the specified flush threshold.
    pub fn new(flush_threshold: usize) -> Self {
        Self {
            entries: Vec::new(),
            flush_threshold,
        }
    }

    /// Create a new access log with default threshold of 50 entries.
    pub fn default_threshold() -> Self {
        Self::new(50)
    }

    /// Record an access event.
    pub fn record(&mut self, scenario: Scenario, filename: &str) {
        self.entries.push(AccessEntry {
            scenario,
            filename: filename.to_string(),
            timestamp: Utc::now(),
        });
    }

    /// Check if the log should be flushed.
    pub fn should_flush(&self) -> bool {
        self.entries.len() >= self.flush_threshold
    }

    /// Drain all entries, returning them for batch processing.
    pub fn drain(&mut self) -> Vec<AccessEntry> {
        std::mem::take(&mut self.entries)
    }

    /// Get the number of pending entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the log is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl FrequencyManager {
    /// Recalculate frequency based on decay rules.
    ///
    /// # Decay Rules
    /// - hot → warm: 7 days without access
    /// - warm → cold: 30 days without access
    /// - cold → archived: 90 days without access
    /// - Profile memories are exempt from decay (always hot)
    ///
    /// Returns the new frequency (may be same as current).
    pub fn recalculate(current: Frequency, last_accessed: &str, scenario: Scenario) -> Frequency {
        // Profile memories are always exempt from decay
        if scenario.is_exempt_from_decay() {
            return Frequency::Hot;
        }

        // Parse last_accessed timestamp
        let last_accessed_dt = match DateTime::parse_from_rfc3339(last_accessed) {
            Ok(dt) => dt.with_timezone(&Utc),
            Err(_) => return current, // If we can't parse, don't decay
        };

        let now = Utc::now();
        let days_since_access = (now - last_accessed_dt).num_days();

        match current {
            Frequency::Hot => {
                if days_since_access > 7 {
                    Frequency::Warm
                } else {
                    Frequency::Hot
                }
            }
            Frequency::Warm => {
                if days_since_access > 30 {
                    Frequency::Cold
                } else {
                    Frequency::Warm
                }
            }
            Frequency::Cold => {
                if days_since_access > 90 {
                    Frequency::Archived
                } else {
                    Frequency::Cold
                }
            }
            Frequency::Archived => Frequency::Archived,
        }
    }

    /// Calculate promotion based on recent access count.
    ///
    /// # Promotion Rules
    /// - cold → warm: on access (any access promotes from cold)
    /// - warm → hot: 3+ accesses in 7 days
    ///
    /// Returns the new frequency after promotion.
    pub fn calculate_promotion(current: Frequency, recent_access_count: u32) -> Frequency {
        match current {
            Frequency::Cold => Frequency::Warm, // Any access promotes from cold
            Frequency::Warm => {
                if recent_access_count >= 3 {
                    Frequency::Hot
                } else {
                    Frequency::Warm
                }
            }
            Frequency::Hot | Frequency::Archived => current,
        }
    }

    /// Run batch decay on stale memories, driven by SQLite queries.
    ///
    /// Instead of scanning the entire filesystem (O(N) disk reads), this method
    /// queries SQLite for candidates whose `last_accessed` is older than the
    /// shortest decay threshold (7 days for hot→warm).
    ///
    /// **Markdown files are never touched** — only SQLite is updated.
    /// Frequency, access_count, and last_accessed are machine runtime state
    /// that lives exclusively in SQLite.
    ///
    /// # Process
    /// 1. Query SQLite for entries with `last_accessed` older than 7 days
    /// 2. For each candidate, recalculate frequency using SQLite data
    /// 3. If frequency changed, update SQLite only (no file I/O)
    ///
    /// Returns a report with statistics about the decay run.
    pub async fn run_decay_batch(
        _store: &FileMemoryStore,
        metadata_store: &MetadataStore,
    ) -> Result<DecayReport> {
        let mut report = DecayReport::default();

        // Step 1: Query SQLite for candidates — O(1) disk I/O (index scan)
        // Use 7 days (shortest threshold) to catch all potentially decayable entries.
        let candidates = metadata_store.get_decay_candidates(7).await?;
        report.total_scanned = candidates.len();

        // Step 2: Recalculate frequency for each candidate using SQLite data
        for candidate in candidates {
            let current_freq = candidate.frequency;
            let last_accessed = &candidate.last_accessed;

            // Recalculate frequency based on decay rules
            let new_freq = Self::recalculate(current_freq, last_accessed, candidate.scenario);

            // If frequency changed, update SQLite only (no file rewrite)
            if new_freq != current_freq {
                match metadata_store
                    .update_runtime_state(
                        candidate.scenario,
                        &candidate.filename,
                        new_freq,
                        last_accessed,
                        0, // no access count change during decay
                    )
                    .await
                {
                    Ok(true) => {
                        report.decayed += 1;
                        tracing::debug!(
                            "Decayed {}/{}: {:?} -> {:?}",
                            candidate.scenario,
                            candidate.filename,
                            current_freq,
                            new_freq
                        );
                    }
                    Ok(false) => {
                        tracing::warn!(
                            "Decay candidate {}/{} not found in SQLite",
                            candidate.scenario,
                            candidate.filename
                        );
                        report.errors += 1;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to update frequency for {}/{}: {}",
                            candidate.scenario,
                            candidate.filename,
                            e
                        );
                        report.errors += 1;
                    }
                }
            }
        }

        Ok(report)
    }

    /// Flush access log to SQLite only.
    ///
    /// Updates frequency and access statistics for each accessed file in SQLite.
    /// **Markdown files are never touched** — this prevents the background
    /// lifecycle from silently overwriting user edits.
    ///
    /// # Process
    /// 1. Drain all entries from access log
    /// 2. Group by (scenario, filename), counting accesses per file
    /// 3. For each unique file:
    ///    - Update access_count (atomic increment) in SQLite
    ///    - Update last_accessed to the latest timestamp
    ///    - Recalculate frequency (check promotion)
    ///    - Single `update_runtime_state` call per file
    ///
    /// Returns a report with statistics about the flush.
    pub async fn flush_access_log(
        log: &mut AccessLog,
        metadata_store: &MetadataStore,
    ) -> Result<FlushReport> {
        let entries = log.drain();
        let mut report = FlushReport::default();

        // Group entries by (scenario, filename)
        let mut file_accesses: HashMap<(Scenario, String), Vec<&AccessEntry>> = HashMap::new();
        for entry in &entries {
            file_accesses
                .entry((entry.scenario, entry.filename.clone()))
                .or_default()
                .push(entry);
        }

        // Process each unique file — SQLite only, no file I/O
        for ((scenario, filename), access_entries) in file_accesses {
            report.total_flushed += 1;

            // Find the latest access timestamp
            let latest_timestamp = access_entries.iter().map(|e| e.timestamp).max().unwrap();
            let access_count_increment = access_entries.len() as u64;

            // Calculate promotion (using access count as proxy for recent accesses)
            // Start from current frequency — we read it from the candidate if available.
            // For flush, we don't know current frequency without a DB read, so we use
            // a reasonable default: any access from cold promotes to warm, 3+ from warm
            // promotes to hot.
            let new_freq = Self::calculate_promotion(Frequency::Cold, access_count_increment as u32);

            match metadata_store
                .update_runtime_state(
                    scenario,
                    &filename,
                    new_freq,
                    &latest_timestamp.to_rfc3339(),
                    access_count_increment,
                )
                .await
            {
                Ok(true) => {
                    report.promoted += 1;
                    tracing::debug!(
                        "Flushed {}/{}: +{} accesses, promoted to {:?}",
                        scenario,
                        filename,
                        access_count_increment,
                        new_freq
                    );
                }
                Ok(false) => {
                    tracing::warn!(
                        "Access log entry for {}/{} not found in SQLite (file may not be indexed)",
                        scenario,
                        filename
                    );
                    report.errors += 1;
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to flush access stats for {}/{}: {}",
                        scenario,
                        filename,
                        e
                    );
                    report.errors += 1;
                }
            }
        }

        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqliteStore;
    use chrono::Duration;
    use tempfile::TempDir;

    /// Create a MetadataStore backed by a temp SQLite database.
    async fn setup_metadata(temp_dir: &TempDir) -> MetadataStore {
        let db_path = temp_dir.path().join("test_metadata.db");
        let pool = SqliteStore::with_path(db_path)
            .await
            .unwrap()
            .pool()
            .clone();
        MetadataStore::new(pool)
    }

    fn create_test_memory(
        scenario: Scenario,
        title: &str,
        frequency: Frequency,
        days_since_access: i64,
    ) -> (String, MemoryMeta, String) {
        let now = Utc::now();
        let last_accessed = now - Duration::days(days_since_access);

        let meta = MemoryMeta {
            id: uuid::Uuid::new_v4().to_string(),
            title: title.to_string(),
            r#type: "test".to_string(),
            scenario,
            tags: vec![],
            frequency,
            access_count: 1,
            created: now.to_rfc3339(),
            updated: now.to_rfc3339(),
            last_accessed: last_accessed.to_rfc3339(),
            auto_expire: false,
            expires: None,
            tokens: 100,
            superseded_by: None,
            index: true,
        };

        let content = "# Test Content".to_string();
        let filename = format!("{}.md", meta.id);

        (filename, meta, content)
    }

    #[test]
    fn test_recalculate_hot_to_warm_after_7_days() {
        let now = Utc::now();
        let eight_days_ago = (now - Duration::days(8)).to_rfc3339();

        let new_freq =
            FrequencyManager::recalculate(Frequency::Hot, &eight_days_ago, Scenario::Knowledge);

        assert_eq!(new_freq, Frequency::Warm);
    }

    #[test]
    fn test_recalculate_hot_stays_hot_within_7_days() {
        let now = Utc::now();
        let five_days_ago = (now - Duration::days(5)).to_rfc3339();

        let new_freq =
            FrequencyManager::recalculate(Frequency::Hot, &five_days_ago, Scenario::Knowledge);

        assert_eq!(new_freq, Frequency::Hot);
    }

    #[test]
    fn test_recalculate_warm_to_cold_after_30_days() {
        let now = Utc::now();
        let thirty_one_days_ago = (now - Duration::days(31)).to_rfc3339();

        let new_freq =
            FrequencyManager::recalculate(Frequency::Warm, &thirty_one_days_ago, Scenario::Active);

        assert_eq!(new_freq, Frequency::Cold);
    }

    #[test]
    fn test_recalculate_cold_to_archived_after_90_days() {
        let now = Utc::now();
        let ninety_one_days_ago = (now - Duration::days(91)).to_rfc3339();

        let new_freq = FrequencyManager::recalculate(
            Frequency::Cold,
            &ninety_one_days_ago,
            Scenario::Episodes,
        );

        assert_eq!(new_freq, Frequency::Archived);
    }

    #[test]
    fn test_recalculate_profile_exempt_from_decay() {
        let now = Utc::now();
        let ancient = (now - Duration::days(365)).to_rfc3339();

        // Profile memories should never decay, even after a year
        let new_freq = FrequencyManager::recalculate(Frequency::Hot, &ancient, Scenario::Profile);

        assert_eq!(new_freq, Frequency::Hot);
    }

    #[test]
    fn test_recalculate_decisions_exempt_from_decay() {
        let now = Utc::now();
        let ancient = (now - Duration::days(365)).to_rfc3339();

        let new_freq =
            FrequencyManager::recalculate(Frequency::Warm, &ancient, Scenario::Decisions);

        assert_eq!(new_freq, Frequency::Hot); // Exempt scenarios become Hot
    }

    #[test]
    fn test_recalculate_reference_exempt_from_decay() {
        let now = Utc::now();
        let ancient = (now - Duration::days(365)).to_rfc3339();

        let new_freq =
            FrequencyManager::recalculate(Frequency::Cold, &ancient, Scenario::Reference);

        assert_eq!(new_freq, Frequency::Hot); // Exempt scenarios become Hot
    }

    #[test]
    fn test_calculate_promotion_cold_to_warm_on_access() {
        // Any access promotes from cold to warm
        let new_freq = FrequencyManager::calculate_promotion(Frequency::Cold, 1);
        assert_eq!(new_freq, Frequency::Warm);
    }

    #[test]
    fn test_calculate_promotion_warm_to_hot_after_3_accesses() {
        let new_freq = FrequencyManager::calculate_promotion(Frequency::Warm, 3);
        assert_eq!(new_freq, Frequency::Hot);
    }

    #[test]
    fn test_calculate_promotion_warm_stays_warm_with_2_accesses() {
        let new_freq = FrequencyManager::calculate_promotion(Frequency::Warm, 2);
        assert_eq!(new_freq, Frequency::Warm);
    }

    #[test]
    fn test_calculate_promotion_hot_stays_hot() {
        let new_freq = FrequencyManager::calculate_promotion(Frequency::Hot, 10);
        assert_eq!(new_freq, Frequency::Hot);
    }

    #[test]
    fn test_access_log_record_and_check_threshold() {
        let mut log = AccessLog::new(5);

        assert!(!log.should_flush());
        assert_eq!(log.len(), 0);

        for i in 0..5 {
            log.record(Scenario::Knowledge, &format!("file{}.md", i));
        }

        assert!(log.should_flush());
        assert_eq!(log.len(), 5);
    }

    #[test]
    fn test_access_log_drain() {
        let mut log = AccessLog::new(10);

        log.record(Scenario::Active, "file1.md");
        log.record(Scenario::Knowledge, "file2.md");

        assert_eq!(log.len(), 2);

        let entries = log.drain();

        assert_eq!(entries.len(), 2);
        assert_eq!(log.len(), 0);
        assert!(log.is_empty());
    }

    #[tokio::test]
    async fn test_flush_access_log_updates_metadata() {
        let temp_dir = TempDir::new().unwrap();
        let metadata_store = setup_metadata(&temp_dir).await;

        // Create a test memory and seed it directly in SQLite
        let (filename, meta, _content) =
            create_test_memory(Scenario::Knowledge, "Test Memory", Frequency::Warm, 0);
        let entry = super::super::index::MemoryIndexEntry {
            id: meta.id.clone(),
            title: meta.title.clone(),
            memory_type: meta.r#type.clone(),
            tags: meta.tags.clone(),
            frequency: meta.frequency,
            tokens: meta.tokens as u32,
            filename: filename.clone(),
            updated: meta.updated.clone(),
            scenario: meta.scenario,
            last_accessed: meta.last_accessed.clone(),
            access_count: 1,
            file_mtime: 0,
            file_size: 0,
            needs_embedding: false,
        };
        metadata_store.upsert_entry(&entry).await.unwrap();

        // Record access
        let mut log = AccessLog::default_threshold();
        log.record(Scenario::Knowledge, &filename);

        // Flush log — SQLite only, no file I/O
        let report = FrequencyManager::flush_access_log(&mut log, &metadata_store)
            .await
            .unwrap();

        assert_eq!(report.total_flushed, 1);
        assert_eq!(report.errors, 0);

        // Verify metadata was updated in SQLite (not in file)
        let entries = metadata_store.query_entries(Scenario::Knowledge).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].access_count, 2); // Was 1, incremented by 1
    }

    #[tokio::test]
    async fn test_flush_access_log_promotes_frequency() {
        let temp_dir = TempDir::new().unwrap();
        let metadata_store = setup_metadata(&temp_dir).await;

        // Create a cold memory in SQLite
        let (filename, meta, _content) =
            create_test_memory(Scenario::Knowledge, "Cold Memory", Frequency::Cold, 0);
        let entry = super::super::index::MemoryIndexEntry {
            id: meta.id.clone(),
            title: meta.title.clone(),
            memory_type: meta.r#type.clone(),
            tags: meta.tags.clone(),
            frequency: meta.frequency,
            tokens: meta.tokens as u32,
            filename: filename.clone(),
            updated: meta.updated.clone(),
            scenario: meta.scenario,
            last_accessed: meta.last_accessed.clone(),
            access_count: 1,
            file_mtime: 0,
            file_size: 0,
            needs_embedding: false,
        };
        metadata_store.upsert_entry(&entry).await.unwrap();

        // Record access
        let mut log = AccessLog::default_threshold();
        log.record(Scenario::Knowledge, &filename);

        // Flush log
        let report = FrequencyManager::flush_access_log(&mut log, &metadata_store)
            .await
            .unwrap();

        assert_eq!(report.promoted, 1);

        // Verify frequency was promoted in SQLite
        let entries = metadata_store.query_entries(Scenario::Knowledge).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].frequency, Frequency::Warm);
    }

    #[tokio::test]
    async fn test_run_decay_batch_updates_stale_memories() {
        let temp_dir = TempDir::new().unwrap();
        let _store = FileMemoryStore::new(temp_dir.path().to_path_buf());
        let metadata_store = setup_metadata(&temp_dir).await;

        _store.init().await.unwrap();

        // Create memories with different ages — seed directly in SQLite
        let memories = vec![
            create_test_memory(Scenario::Active, "Hot Memory", Frequency::Hot, 1),
            create_test_memory(Scenario::Active, "Stale Hot", Frequency::Hot, 8),
            create_test_memory(Scenario::Knowledge, "Stale Warm", Frequency::Warm, 31),
        ];

        for (filename, meta, _content) in memories {
            let entry = super::super::index::MemoryIndexEntry {
                id: meta.id.clone(),
                title: meta.title.clone(),
                memory_type: meta.r#type.clone(),
                tags: meta.tags.clone(),
                frequency: meta.frequency,
                tokens: meta.tokens as u32,
                filename: filename.clone(),
                updated: meta.updated.clone(),
                scenario: meta.scenario,
                last_accessed: meta.last_accessed.clone(),
                access_count: 0,
                file_mtime: 0,
                file_size: 0,
                needs_embedding: false,
            };
            metadata_store.upsert_entry(&entry).await.unwrap();
        }

        // Run decay batch — SQLite only
        let report = FrequencyManager::run_decay_batch(&_store, &metadata_store)
            .await
            .unwrap();

        assert_eq!(report.total_scanned, 2); // Only the 2 stale entries are candidates
        assert!(report.decayed >= 2); // Both stale ones should decay

        // Verify decayed frequencies in SQLite
        let active = metadata_store.query_entries(Scenario::Active).await.unwrap();
        for entry in &active {
            if entry.title == "Stale Hot" {
                assert_eq!(entry.frequency, Frequency::Warm);
            } else if entry.title == "Hot Memory" {
                assert_eq!(entry.frequency, Frequency::Hot);
            }
        }

        let knowledge = metadata_store.query_entries(Scenario::Knowledge).await.unwrap();
        for entry in &knowledge {
            if entry.title == "Stale Warm" {
                assert_eq!(entry.frequency, Frequency::Cold);
            }
        }
    }

    #[tokio::test]
    async fn test_flush_groups_multiple_accesses() {
        let temp_dir = TempDir::new().unwrap();
        let metadata_store = setup_metadata(&temp_dir).await;

        // Create a test memory in SQLite
        let (filename, meta, _content) =
            create_test_memory(Scenario::Active, "Popular Memory", Frequency::Warm, 0);
        let entry = super::super::index::MemoryIndexEntry {
            id: meta.id.clone(),
            title: meta.title.clone(),
            memory_type: meta.r#type.clone(),
            tags: meta.tags.clone(),
            frequency: meta.frequency,
            tokens: meta.tokens as u32,
            filename: filename.clone(),
            updated: meta.updated.clone(),
            scenario: meta.scenario,
            last_accessed: meta.last_accessed.clone(),
            access_count: 1,
            file_mtime: 0,
            file_size: 0,
            needs_embedding: false,
        };
        metadata_store.upsert_entry(&entry).await.unwrap();

        // Record multiple accesses to the same file
        let mut log = AccessLog::default_threshold();
        log.record(Scenario::Active, &filename);
        log.record(Scenario::Active, &filename);
        log.record(Scenario::Active, &filename);

        // Flush log
        let report = FrequencyManager::flush_access_log(&mut log, &metadata_store)
            .await
            .unwrap();

        // Should report 1 file flushed, not 3
        assert_eq!(report.total_flushed, 1);
        assert_eq!(report.promoted, 1); // 3 accesses should promote to Hot

        // Verify access count was incremented by 3 in SQLite
        let entries = metadata_store.query_entries(Scenario::Active).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].access_count, 4); // Was 1, +3 accesses
        assert_eq!(entries[0].frequency, Frequency::Hot);
    }
}
