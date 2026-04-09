//! File watcher for memory directory changes - Manual refresh version.
//!
//! Provides utilities for manual refresh of memory files.
//! Call `refresh_all_files()` to detect external file changes by comparing mtime and size.
//!
//! # Filtering
//!
//! The following are ignored:
//! - `.history/` directory (version-controlled backups)
//! - `.tmp` files (temporary editor files)
//! - `README.md` files (human-written notes, not memory entries)
//! - Dotfiles (hidden files starting with `.`)

//!
//! # Auto-Indexing
//!
//! The `AutoIndexHandler` provides manual refresh functionality.
//! Call `process_file()` to manually sync a file's metadata and embedding to SQLite.

use super::types::Scenario;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Refresh report from manual refresh operations
#[derive(Debug, Clone)]
pub struct RefreshReport {
    pub processed: usize,
    pub updated: usize,
    pub skipped: usize,
    pub errors: usize,
}

/// Check if a file path should be ignored by the watcher.
///
/// Filters out:
/// - `.history/` directory (version history)
/// - `.tmp` files (temporary editor files)
/// - `README.md` files (human-written notes)
/// - Dotfiles (hidden files starting with `.`)
pub fn should_ignore(path: &Path) -> bool {
    let path_str = path.to_string_lossy();

    // Ignore .history directory
    if path_str.contains("/.history/")
        || path_str.contains("\\.history\\")
        || path_str.starts_with(".history/")
        || path_str.starts_with(".history\\")
    {
        return true;
    }

    // Ignore .tmp files
    if path_str.ends_with(".tmp") {
        return true;
    }

    // Ignore README.md files (human-written notes, not memory entries)
    if path.ends_with("README.md") {
        return true;
    }

    // Ignore dotfiles
    if path
        .file_name()
        .map(|n| n.to_string_lossy().starts_with('.'))
        .unwrap_or(false)
    {
        return true;
    }

    false
}

/// Extract scenario from a file path relative to memory base dir.
pub fn scenario_from_path(path: &Path) -> Option<Scenario> {
    path.iter()
        .next()
        .and_then(|s| s.to_str())
        .and_then(Scenario::from_dir_name)
}

// ============================================================================
// Auto-index handler - Manual refresh version
// ============================================================================

/// Handler that provides manual refresh functionality for memory files.
///
/// When a `.md` file is refreshed manually:
/// - Parses its YAML frontmatter
/// - UPSERTs metadata into the `memory_metadata` SQLite table
/// - UPSERTs embedding into the `memory_embeddings` SQLite table
pub struct AutoIndexHandler {
    metadata_store: super::metadata_store::MetadataStore,
    embedding_store: super::embedding_store::EmbeddingStore,
    base_dir: PathBuf,
    embedder: Arc<dyn super::embedder::Embedder>,
}

impl AutoIndexHandler {
    /// Create a new auto-index handler.
    pub fn new(
        metadata_store: super::metadata_store::MetadataStore,
        embedding_store: super::embedding_store::EmbeddingStore,
        base_dir: PathBuf,
        embedder: Arc<dyn super::embedder::Embedder>,
    ) -> Self {
        Self {
            metadata_store,
            embedding_store,
            base_dir,
            embedder,
        }
    }

    /// Refresh all memory files from disk, comparing mtime and size to detect changes.
    ///
    /// This is the manual replacement for the file watcher - call this method
    /// when you suspect external file changes may have occurred.
    pub async fn refresh_all_files(&self) -> Result<RefreshReport> {
        let mut report = RefreshReport {
            processed: 0,
            updated: 0,
            skipped: 0,
            errors: 0,
        };

        for scenario in Scenario::all() {
            let scenario_dir = self.base_dir.join(scenario.dir_name());
            if !scenario_dir.exists() {
                continue;
            }

            for entry in std::fs::read_dir(&scenario_dir)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
            {
                let path = entry.path();
                let ext = path.extension().and_then(|s| s.to_str());

                if ext != Some("md") {
                    continue;
                }

                if should_ignore(&path) {
                    continue;
                }

                report.processed += 1;

                // Read file metadata
                let Ok(metadata) = std::fs::metadata(&path) else {
                    report.errors += 1;
                    continue;
                };

                let disk_mtime = metadata
                    .modified()
                    .ok()
                    .and_then(|d| d.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0);
                let disk_size = metadata.len();

                let filename = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();

                // Query SQLite for stored mtime and size
                let (sqlite_mtime, sqlite_size) = self
                    .metadata_store
                    .get_file_mtime_and_size(*scenario, &filename)
                    .await
                    .unwrap_or((0, 0));

                // Skip if mtime and size match (no changes)
                if disk_mtime <= sqlite_mtime && disk_size == sqlite_size {
                    report.skipped += 1;
                    continue;
                }

                // Process file
                report.updated += 1;
                if (self.process_file(&path, *scenario).await).is_err() {
                    report.errors += 1;
                }
            }
        }

        Ok(report)
    }

    /// Process a single file (O(1) — only touches the specified file).
    ///
    /// 1. Parse YAML frontmatter
    /// 2. UPSERT metadata into the `memory_metadata` SQLite table
    /// 3. UPSERT embedding into the `memory_embeddings` SQLite table
    pub async fn process_file(&self, path: &Path, scenario: Scenario) -> Result<()> {
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        // Read disk metadata (mtime + size)
        let (disk_mtime, disk_size) = match tokio::fs::metadata(path).await {
            Ok(m) => {
                let mtime = m
                    .modified()
                    .ok()
                    .and_then(|d| d.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0);
                let size = m.len();
                (mtime, size)
            }
            Err(_) => {
                tracing::debug!("AutoIndex: cannot read metadata for {:?}", path);
                return Ok(());
            }
        };

        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(_) => return Ok(()),
        };

        match super::frontmatter::parse_memory_file(&content) {
            Ok((meta, _)) => {
                // O(1) SQLite upsert
                let entry = super::index::MemoryIndexEntry {
                    id: meta.id,
                    title: meta.title,
                    memory_type: meta.r#type,
                    tags: meta.tags.clone(),
                    frequency: meta.frequency,
                    tokens: meta.tokens as u32,
                    filename: filename.clone(),
                    updated: meta.updated,
                    scenario,
                    last_accessed: meta.last_accessed.clone(),
                    file_mtime: disk_mtime,
                    file_size: disk_size,
                    needs_embedding: true,
                };

                if let Err(e) = self.metadata_store.upsert_entry(&entry).await {
                    tracing::error!("AutoIndex: failed to upsert metadata: {}", e);
                }

                let embedding = match self.embedder.embed(&content).await {
                    Ok(e) => e,
                    Err(err) => {
                        tracing::warn!("AutoIndex: embed failed for {}: {}", filename, err);
                        return Ok(());
                    }
                };
                if let Err(e) = self
                    .embedding_store
                    .upsert(
                        &filename,
                        scenario.dir_name(),
                        &meta.tags,
                        meta.frequency,
                        &embedding,
                        meta.tokens as u32,
                    )
                    .await
                {
                    tracing::error!("AutoIndex: failed to upsert embedding: {}", e);
                }

                // Mark embedding as complete
                if let Err(e) = self
                    .metadata_store
                    .mark_embedding_done(scenario, &filename)
                    .await
                {
                    tracing::warn!("AutoIndex: failed to clear needs_embedding: {}", e);
                }

                tracing::debug!("AutoIndex: processed {}", filename);
            }
            Err(e) => {
                tracing::warn!(
                    "AutoIndex: broken frontmatter in {}/{}: {} — skipping",
                    scenario.dir_name(),
                    filename,
                    e
                );
            }
        }

        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_ignore_history() {
        assert!(should_ignore(Path::new("knowledge/.history/old.md")));
        assert!(should_ignore(Path::new(".history/file.md")));
        assert!(should_ignore(Path::new("knowledge\\.history\\old.md")));
    }

    #[test]
    fn test_should_ignore_tmp() {
        assert!(should_ignore(Path::new("active/temp.tmp")));
        assert!(should_ignore(Path::new(".tmp")));
        assert!(should_ignore(Path::new("file.txt.tmp")));
    }

    #[test]
    fn test_should_ignore_readme() {
        assert!(should_ignore(Path::new("knowledge/README.md")));
        assert!(should_ignore(Path::new("README.md")));
        assert!(should_ignore(Path::new("decisions/README.md")));
    }

    #[test]
    fn test_should_ignore_dotfiles() {
        assert!(should_ignore(Path::new(".hidden.md")));
        assert!(should_ignore(Path::new("knowledge/.secret.md")));
        assert!(should_ignore(Path::new(".DS_Store")));
    }

    #[test]
    fn test_should_not_ignore_regular_files() {
        assert!(!should_ignore(Path::new("knowledge/rust.md")));
        assert!(!should_ignore(Path::new("active/task.md")));
        assert!(!should_ignore(Path::new("decisions/choice.md")));
    }

    #[test]
    fn test_scenario_from_path() {
        let path = PathBuf::from("knowledge/rust-async.md");
        assert_eq!(Some(Scenario::Knowledge), scenario_from_path(&path));

        let path = PathBuf::from("profile/user-info.md");
        assert_eq!(Some(Scenario::Profile), scenario_from_path(&path));

        let path = PathBuf::from("active/current-task.md");
        assert_eq!(Some(Scenario::Active), scenario_from_path(&path));

        let path = PathBuf::from("decisions/arch-choice.md");
        assert_eq!(Some(Scenario::Decisions), scenario_from_path(&path));

        let path = PathBuf::from("episodes/conversation.md");
        assert_eq!(Some(Scenario::Episodes), scenario_from_path(&path));

        let path = PathBuf::from("reference/doc-link.md");
        assert_eq!(Some(Scenario::Reference), scenario_from_path(&path));

        let path = PathBuf::from("invalid/file.md");
        assert_eq!(None, scenario_from_path(&path));

        let path = PathBuf::from("");
        assert_eq!(None, scenario_from_path(&path));

        let path = PathBuf::from("file.md");
        assert_eq!(None, scenario_from_path(&path));
    }
}
