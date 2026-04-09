//! Directory scanner for memory file metadata.
//!
//! Provides `scan_entries()` to read YAML frontmatter from all `.md` files in
//! a scenario directory and return parsed `MemoryIndexEntry` structs.
//!
//! The old `_INDEX.md` materialized-view file has been removed. All metadata
//! queries go through the SQLite-backed `MetadataStore`, which is kept in
//! sync by `scan_entries()` + `sync_entries()` at startup and by O(1)
//! upserts from the file watcher.

use super::frontmatter::*;
use super::types::*;
use anyhow::Result;
use std::path::PathBuf;

/// A single entry representing a memory file's metadata.
#[derive(Debug, Clone)]
pub struct MemoryIndexEntry {
    pub id: String,
    pub title: String,
    pub memory_type: String,
    pub tags: Vec<String>,
    pub frequency: Frequency,
    pub tokens: u32,
    pub filename: String,
    pub updated: String,
    pub scenario: Scenario,
    pub last_accessed: String,
    pub file_mtime: u64,
    /// File size in bytes, used for cache invalidation alongside mtime
    pub file_size: u64,
    /// Whether this entry still needs an embedding vector computed.
    /// Set to `true` on creation, `false` after successful embedding.
    pub needs_embedding: bool,
}

/// Scanner for memory file metadata within a scenario directory.
///
/// Reads YAML frontmatter directly from `.md` files — no intermediate
/// `_INDEX.md` is generated or consulted.
pub struct FileIndexManager {
    base_dir: PathBuf,
}

impl FileIndexManager {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    pub fn default_path() -> Self {
        Self::new(super::path::memory_base_dir())
    }

    /// Scan `.md` files in a scenario directory and return parsed metadata entries.
    ///
    /// Reads YAML frontmatter directly from each file. Results are sorted by
    /// frequency (hot first, then warm, then cold, then archived).
    pub async fn scan_entries(&self, scenario: Scenario) -> Result<Vec<MemoryIndexEntry>> {
        let dir = self.base_dir.join(scenario.dir_name());
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut entries = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&dir).await?;

        while let Some(entry) = read_dir.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".md") || name.starts_with('.') || name == "README.md" {
                continue;
            }

            let path = entry.path();
            let file_mtime = tokio::fs::metadata(&path)
                .await
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|d| d.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            match tokio::fs::read_to_string(&path).await {
                Ok(content) => {
                    match parse_memory_file(&content) {
                        Ok((meta, _)) => {
                            // Get file size for cache invalidation
                            let file_size = tokio::fs::metadata(&path)
                                .await
                                .map(|m| m.len())
                                .unwrap_or(0);

                            entries.push(MemoryIndexEntry {
                                id: meta.id,
                                title: meta.title,
                                memory_type: meta.r#type,
                                tags: meta.tags,
                                frequency: meta.frequency,
                                tokens: meta.tokens as u32,
                                filename: name,
                                updated: meta.updated,
                                scenario,
                                last_accessed: meta.last_accessed,
                                file_mtime,
                                file_size,
                                needs_embedding: true,
                            });
                        }
                        Err(e) => {
                            // Broken frontmatter — log warning and skip.
                            // Don't insert fake data into the database.
                            tracing::warn!(
                                "Broken frontmatter in {}/{}: {} — skipping",
                                scenario.dir_name(),
                                name,
                                e
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Skipping unreadable memory file {}: {}", name, e);
                }
            }
        }

        // Sort: hot first, then warm, then cold, then archived
        entries.sort_by(|a, b| b.frequency.cmp(&a.frequency));
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_store(temp_dir: &PathBuf) -> FileIndexManager {
        FileIndexManager::new(temp_dir.clone())
    }

    #[tokio::test]
    async fn test_scan_entries_bypasses_index() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_test_store(&temp_dir.path().to_path_buf());

        // Create scenario directory
        let dir = temp_dir.path().join("knowledge");
        tokio::fs::create_dir_all(&dir).await.unwrap();

        // Create memory files
        let memory_content = r#"---
id: mem_scan_hot
title: Hot Memory
type: note
scenario: knowledge
tags:
  - test
frequency: hot
created: 2026-04-03T00:00:00Z
updated: 2026-04-03T00:00:00Z
last_accessed: 2026-04-03T00:00:00Z
tokens: 100
---
"#;
        tokio::fs::write(dir.join("hot.md"), memory_content)
            .await
            .unwrap();

        let memory_cold = r#"---
id: mem_scan_cold
title: Cold Memory
type: note
scenario: knowledge
tags:
  - old
frequency: cold
created: 2026-04-01T00:00:00Z
updated: 2026-04-01T00:00:00Z
last_accessed: 2026-04-01T00:00:00Z
tokens: 50
---
"#;
        tokio::fs::write(dir.join("cold.md"), memory_cold)
            .await
            .unwrap();

        let entries = store.scan_entries(Scenario::Knowledge).await.unwrap();
        assert_eq!(2, entries.len());

        // Hot should come first (sorted by frequency)
        assert_eq!("Hot Memory", entries[0].title);
        assert_eq!(Frequency::Hot, entries[0].frequency);
        assert_eq!("Cold Memory", entries[1].title);
        assert_eq!(Frequency::Cold, entries[1].frequency);
    }

    #[tokio::test]
    async fn test_scan_entries_skips_readme_and_dotfiles() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_test_store(&temp_dir.path().to_path_buf());

        let dir = temp_dir.path().join("knowledge");
        tokio::fs::create_dir_all(&dir).await.unwrap();

        // Valid memory
        let memory = r#"---
id: mem_valid
title: Valid
type: note
scenario: knowledge
frequency: warm
created: 2026-04-03T00:00:00Z
updated: 2026-04-03T00:00:00Z
last_accessed: 2026-04-03T00:00:00Z
tokens: 50
---
"#;
        tokio::fs::write(dir.join("valid.md"), memory)
            .await
            .unwrap();

        // README.md should be skipped
        tokio::fs::write(dir.join("README.md"), "# My notes")
            .await
            .unwrap();

        // Dotfile should be skipped
        tokio::fs::write(dir.join(".hidden.md"), "hidden")
            .await
            .unwrap();

        let entries = store.scan_entries(Scenario::Knowledge).await.unwrap();
        assert_eq!(1, entries.len());
        assert_eq!("Valid", entries[0].title);
    }
}
