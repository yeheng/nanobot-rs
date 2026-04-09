//! Frequency lifecycle management for memory files.
//!
//! This module implements automatic decay and promotion of memory frequency tiers,
//! along with deferred batched access tracking for performance optimization.

use super::frontmatter::*;
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
    /// shortest decay threshold (7 days for hot→warm). Only those O(k) files
    /// are read from disk and potentially rewritten.
    ///
    /// # Process
    /// 1. Query SQLite for entries with `last_accessed` older than 7 days
    /// 2. For each candidate, read the .md file and recalculate frequency
    /// 3. If frequency changed, rewrite frontmatter + O(1) SQLite upsert
    ///
    /// Returns a report with statistics about the decay run.
    pub async fn run_decay_batch(
        store: &FileMemoryStore,
        metadata_store: &MetadataStore,
    ) -> Result<DecayReport> {
        let mut report = DecayReport::default();

        // Step 1: Query SQLite for candidates — O(1) disk I/O (index scan)
        // Use 7 days (shortest threshold) to catch all potentially decayable entries.
        let candidates = metadata_store.get_decay_candidates(7).await?;
        report.total_scanned = candidates.len();

        // Step 2: Only read/write the k candidates, not all N files
        for candidate in candidates {
            let scenario = candidate.scenario;
            let filename = &candidate.filename;

            // Read current metadata from file
            let memory_file = match store.read(scenario, filename).await {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(
                        "Failed to read memory file {}/{}: {}",
                        scenario,
                        filename,
                        e
                    );
                    report.errors += 1;
                    continue;
                }
            };

            let current_freq = memory_file.metadata.frequency;
            let last_accessed = &memory_file.metadata.last_accessed;

            // Recalculate frequency based on decay rules
            let new_freq =
                Self::recalculate(current_freq, last_accessed, memory_file.metadata.scenario);

            // If frequency changed, update the file + SQLite
            if new_freq != current_freq {
                let mut updated_meta = memory_file.metadata.clone();
                updated_meta.frequency = new_freq;
                updated_meta.updated = Utc::now().to_rfc3339();

                let new_content = serialize_memory_file(&updated_meta, &memory_file.content);

                if let Err(e) = store.update(scenario, filename, &new_content).await {
                    tracing::warn!(
                        "Failed to update frequency for {}/{}: {}",
                        scenario,
                        filename,
                        e
                    );
                    report.errors += 1;
                } else {
                    report.decayed += 1;

                    // Read file mtime after write
                    let file_path = store.base_dir().join(scenario.dir_name()).join(filename);
                    let (file_mtime, file_size) = tokio::fs::metadata(&file_path)
                        .await
                        .map(|m| {
                            let mtime = m
                                .modified()
                                .ok()
                                .and_then(|d| d.duration_since(std::time::UNIX_EPOCH).ok())
                                .map(|d| d.as_nanos() as u64)
                                .unwrap_or(0);
                            (mtime, m.len())
                        })
                        .unwrap_or((0, 0));

                    // O(1) SQLite upsert for this single entry
                    let entry = super::index::MemoryIndexEntry {
                        id: updated_meta.id,
                        title: updated_meta.title,
                        memory_type: updated_meta.r#type,
                        tags: updated_meta.tags,
                        frequency: updated_meta.frequency,
                        tokens: updated_meta.tokens as u32,
                        filename: filename.clone(),
                        updated: updated_meta.updated,
                        scenario,
                        last_accessed: updated_meta.last_accessed.clone(),
                        file_mtime,
                        file_size,
                        needs_embedding: false,
                    };
                    if let Err(e) = metadata_store.upsert_entry(&entry).await {
                        tracing::warn!(
                            "Failed to upsert metadata for {}/{}: {}",
                            scenario,
                            filename,
                            e
                        );
                        report.errors += 1;
                    }

                    tracing::debug!(
                        "Decayed {}/{}: {:?} -> {:?}",
                        scenario,
                        filename,
                        current_freq,
                        new_freq
                    );
                }
            }
        }

        Ok(report)
    }

    /// Flush access log to disk.
    ///
    /// Updates frontmatter + frequency for each accessed file, then upserts to SQLite.
    ///
    /// # Process
    /// 1. Drain all entries from access log
    /// 2. Group by (scenario, filename), counting accesses per file
    /// 3. For each unique file:
    ///    - Read current content and parse frontmatter
    ///    - Increment access_count by the number of accesses
    ///    - Update last_accessed to the latest timestamp
    ///    - Recalculate frequency (check promotion)
    ///    - Rewrite frontmatter + O(1) SQLite upsert
    ///
    /// Returns a report with statistics about the flush.
    pub async fn flush_access_log(
        log: &mut AccessLog,
        store: &FileMemoryStore,
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

        // Process each unique file
        for ((scenario, filename), access_entries) in file_accesses {
            report.total_flushed += 1;

            // Read current memory file
            let memory_file = match store.read(scenario, &filename).await {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(
                        "Failed to read memory file {}/{}: {}",
                        scenario,
                        filename,
                        e
                    );
                    report.errors += 1;
                    continue;
                }
            };

            // Find the latest access timestamp
            let latest_timestamp = access_entries.iter().map(|e| e.timestamp).max().unwrap();

            let access_count_increment = access_entries.len() as u64;
            let old_freq = memory_file.metadata.frequency;

            // Update metadata
            let mut updated_meta = memory_file.metadata.clone();
            updated_meta.access_count += access_count_increment;
            updated_meta.last_accessed = latest_timestamp.to_rfc3339();
            updated_meta.updated = Utc::now().to_rfc3339();

            // Calculate promotion (using access count as proxy for recent accesses)
            // In a real system, we'd track accesses in a rolling window
            let new_freq = Self::calculate_promotion(old_freq, access_count_increment as u32);
            updated_meta.frequency = new_freq;

            // Check if promotion occurred
            if new_freq > old_freq {
                report.promoted += 1;
                tracing::debug!(
                    "Promoted {}/{}: {:?} -> {:?}",
                    scenario,
                    filename,
                    old_freq,
                    new_freq
                );
            }

            // Write updated file
            let new_content = serialize_memory_file(&updated_meta, &memory_file.content);
            if let Err(e) = store.update(scenario, &filename, &new_content).await {
                tracing::warn!("Failed to update {}/{}: {}", scenario, filename, e);
                report.errors += 1;
                continue;
            }

            // Read file mtime after write
            let file_path = store.base_dir().join(scenario.dir_name()).join(&filename);
            let file_mtime = tokio::fs::metadata(&file_path)
                .await
                .ok()
                .map(|m| {
                    let mtime = m
                        .modified()
                        .ok()
                        .and_then(|d| d.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_nanos() as u64)
                        .unwrap_or(0);
                    (mtime, m.len())
                })
                .unwrap_or((0, 0));

            // O(1) SQLite upsert for this single entry
            let entry = super::index::MemoryIndexEntry {
                id: updated_meta.id,
                title: updated_meta.title,
                memory_type: updated_meta.r#type,
                tags: updated_meta.tags,
                frequency: updated_meta.frequency,
                tokens: updated_meta.tokens as u32,
                filename: filename.clone(),
                updated: updated_meta.updated,
                scenario,
                last_accessed: updated_meta.last_accessed.clone(),
                file_mtime: file_mtime.0,
                file_size: file_mtime.1,
                needs_embedding: false,
            };
            if let Err(e) = metadata_store.upsert_entry(&entry).await {
                tracing::warn!(
                    "Failed to upsert metadata for {}/{}: {}",
                    scenario,
                    filename,
                    e
                );
                report.errors += 1;
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
        let store = FileMemoryStore::new(temp_dir.path().to_path_buf());
        let metadata_store = setup_metadata(&temp_dir).await;

        store.init().await.unwrap();

        // Create a test memory
        let (filename, _meta, content) =
            create_test_memory(Scenario::Knowledge, "Test Memory", Frequency::Warm, 0);
        let scenario_dir = temp_dir.path().join("knowledge");
        tokio::fs::create_dir_all(&scenario_dir).await.unwrap();
        tokio::fs::write(
            scenario_dir.join(&filename),
            serialize_memory_file(&_meta, &content),
        )
        .await
        .unwrap();

        // Record access
        let mut log = AccessLog::default_threshold();
        log.record(Scenario::Knowledge, &filename);

        // Flush log
        let report = FrequencyManager::flush_access_log(&mut log, &store, &metadata_store)
            .await
            .unwrap();

        assert_eq!(report.total_flushed, 1);
        assert_eq!(report.errors, 0);

        // Verify metadata was updated
        let updated = store.read(Scenario::Knowledge, &filename).await.unwrap();
        assert_eq!(updated.metadata.access_count, 2); // Was 1, incremented by 1
        assert!(updated.metadata.last_accessed.len() > 0);
    }

    #[tokio::test]
    async fn test_flush_access_log_promotes_frequency() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileMemoryStore::new(temp_dir.path().to_path_buf());
        let metadata_store = setup_metadata(&temp_dir).await;

        store.init().await.unwrap();

        // Create a cold memory
        let (filename, _meta, content) =
            create_test_memory(Scenario::Knowledge, "Cold Memory", Frequency::Cold, 0);
        let scenario_dir = temp_dir.path().join("knowledge");
        tokio::fs::create_dir_all(&scenario_dir).await.unwrap();
        tokio::fs::write(
            scenario_dir.join(&filename),
            serialize_memory_file(&_meta, &content),
        )
        .await
        .unwrap();

        // Record access
        let mut log = AccessLog::default_threshold();
        log.record(Scenario::Knowledge, &filename);

        // Flush log
        let report = FrequencyManager::flush_access_log(&mut log, &store, &metadata_store)
            .await
            .unwrap();

        assert_eq!(report.promoted, 1);

        // Verify frequency was promoted
        let updated = store.read(Scenario::Knowledge, &filename).await.unwrap();
        assert_eq!(updated.metadata.frequency, Frequency::Warm);
    }

    #[tokio::test]
    async fn test_run_decay_batch_updates_stale_memories() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileMemoryStore::new(temp_dir.path().to_path_buf());
        let metadata_store = setup_metadata(&temp_dir).await;

        store.init().await.unwrap();

        // Create memories with different ages
        let memories = vec![
            create_test_memory(Scenario::Active, "Hot Memory", Frequency::Hot, 1),
            create_test_memory(Scenario::Active, "Stale Hot", Frequency::Hot, 8),
            create_test_memory(Scenario::Knowledge, "Stale Warm", Frequency::Warm, 31),
        ];

        for (filename, meta, content) in memories {
            let scenario_dir = temp_dir.path().join(meta.scenario.dir_name());
            tokio::fs::create_dir_all(&scenario_dir).await.unwrap();
            tokio::fs::write(
                scenario_dir.join(&filename),
                serialize_memory_file(&meta, &content),
            )
            .await
            .unwrap();

            // Sync this entry to SQLite so get_decay_candidates can find it
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
                file_mtime: 0,
                file_size: 0,
                needs_embedding: false,
            };
            metadata_store.upsert_entry(&entry).await.unwrap();
        }

        // Run decay batch
        let report = FrequencyManager::run_decay_batch(&store, &metadata_store)
            .await
            .unwrap();

        assert_eq!(report.total_scanned, 2); // Only the 2 stale entries are candidates
        assert!(report.decayed >= 2); // Both stale ones should decay

        // Verify decayed frequencies
        let files = store.list(Scenario::Active).await.unwrap();
        for filename in files {
            let memory = store.read(Scenario::Active, &filename).await.unwrap();
            if memory.metadata.title == "Stale Hot" {
                assert_eq!(memory.metadata.frequency, Frequency::Warm);
            } else if memory.metadata.title == "Hot Memory" {
                assert_eq!(memory.metadata.frequency, Frequency::Hot);
            }
        }

        let files = store.list(Scenario::Knowledge).await.unwrap();
        for filename in files {
            let memory = store.read(Scenario::Knowledge, &filename).await.unwrap();
            if memory.metadata.title == "Stale Warm" {
                assert_eq!(memory.metadata.frequency, Frequency::Cold);
            }
        }
    }

    #[tokio::test]
    async fn test_flush_groups_multiple_accesses() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileMemoryStore::new(temp_dir.path().to_path_buf());
        let metadata_store = setup_metadata(&temp_dir).await;

        store.init().await.unwrap();

        // Create a test memory
        let (filename, _meta, content) =
            create_test_memory(Scenario::Active, "Popular Memory", Frequency::Warm, 0);
        let scenario_dir = temp_dir.path().join("active");
        tokio::fs::create_dir_all(&scenario_dir).await.unwrap();
        tokio::fs::write(
            scenario_dir.join(&filename),
            serialize_memory_file(&_meta, &content),
        )
        .await
        .unwrap();

        // Record multiple accesses to the same file
        let mut log = AccessLog::default_threshold();
        log.record(Scenario::Active, &filename);
        log.record(Scenario::Active, &filename);
        log.record(Scenario::Active, &filename);

        // Flush log
        let report = FrequencyManager::flush_access_log(&mut log, &store, &metadata_store)
            .await
            .unwrap();

        // Should report 1 file flushed, not 3
        assert_eq!(report.total_flushed, 1);
        assert_eq!(report.promoted, 1); // 3 accesses should promote to Hot

        // Verify access count was incremented by 3
        let updated = store.read(Scenario::Active, &filename).await.unwrap();
        assert_eq!(updated.metadata.access_count, 4); // Was 1, +3 accesses
        assert_eq!(updated.metadata.frequency, Frequency::Hot);
    }
}
