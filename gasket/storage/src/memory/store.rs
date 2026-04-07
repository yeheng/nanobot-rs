use super::frontmatter::*;
use super::path::*;
use super::types::*;
use anyhow::{Context, Result};
use chrono::Utc;
use std::path::PathBuf;

/// Filesystem-backed memory store.
pub struct FileMemoryStore {
    base_dir: PathBuf,
}

/// Max versions kept per memory file.
const MAX_HISTORY_VERSIONS: usize = 10;

impl FileMemoryStore {
    /// Write data to a file atomically using temp-file + rename.
    ///
    /// Writes to a `.tmp` file first, syncs to disk, then renames to the
    /// final path. On POSIX systems (Linux, macOS), `rename` is atomic,
    /// so a crash will never leave a partially-written (corrupted) file.
    async fn atomic_write(path: &std::path::Path, data: impl AsRef<[u8]>) -> Result<()> {
        let tmp_path = path.with_extension("md.tmp");

        // Write to temp file
        tokio::fs::write(&tmp_path, &data)
            .await
            .with_context(|| format!("Failed to write temp file: {}", tmp_path.display()))?;

        // Sync temp file to disk for durability
        #[cfg(unix)]
        {
            let file = tokio::fs::File::open(&tmp_path).await?;

            file.sync_all().await?;
        }

        // Atomic rename: on POSIX, this is an atomic operation
        tokio::fs::rename(&tmp_path, path).await.with_context(|| {
            format!(
                "Failed to rename {} -> {}",
                tmp_path.display(),
                path.display()
            )
        })?;

        Ok(())
    }
    /// Create a store pointing at a base directory (usually ~/.gasket/memory/).
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Get the base directory of this store.
    pub fn base_dir(&self) -> &PathBuf {
        &self.base_dir
    }

    /// Create store using default ~/.gasket/memory/ path.
    pub fn default_path() -> Self {
        Self::new(memory_base_dir())
    }

    /// Initialize directory structure: create all 6 scenario dirs + .history/.
    pub async fn init(&self) -> Result<()> {
        for scenario in Scenario::all() {
            let dir = self.base_dir.join(scenario.dir_name());
            tokio::fs::create_dir_all(&dir).await?;
        }
        // Create .history directory
        let history_base = self.base_dir.join(".history");
        for scenario in Scenario::all() {
            let dir = history_base.join(scenario.dir_name());
            tokio::fs::create_dir_all(&dir).await?;
        }
        Ok(())
    }

    /// Create a new memory file in a scenario.
    /// Returns the generated filename (id-based).
    pub async fn create(
        &self,
        scenario: Scenario,
        title: &str,
        memory_type: &str,
        tags: &[String],
        content: &str,
    ) -> Result<String> {
        let id = format!("mem_{}", uuid::Uuid::now_v7());
        let now = Utc::now().to_rfc3339();
        let tokens = estimate_tokens(content);

        let meta = MemoryMeta {
            id,
            title: title.to_string(),
            r#type: memory_type.to_string(),
            scenario,
            tags: tags.to_vec(),
            frequency: Frequency::Warm,
            access_count: 0,
            created: now.clone(),
            updated: now.clone(),
            last_accessed: now,
            auto_expire: false,
            expires: None,
            tokens: tokens as usize,
            superseded_by: None,
            index: true,
        };

        let filename = format!("{}.md", meta.id);
        let file_content = serialize_memory_file(&meta, content);

        let dir = self.base_dir.join(scenario.dir_name());
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join(&filename);
        Self::atomic_write(&path, file_content)
            .await
            .context("Failed to write memory file")?;

        Ok(filename)
    }

    /// Read a memory file by scenario and filename.
    pub async fn read(&self, scenario: Scenario, filename: &str) -> Result<MemoryFile> {
        let path = self.base_dir.join(scenario.dir_name()).join(filename);
        let content = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("Failed to read memory file: {}", filename))?;
        let (meta, body) = parse_memory_file(&content)?;
        Ok(MemoryFile {
            metadata: meta,
            content: body,
        })
    }

    /// Update an existing memory file. Saves version history before overwriting.
    pub async fn update(
        &self,
        scenario: Scenario,
        filename: &str,
        new_content: &str,
    ) -> Result<()> {
        let path = self.base_dir.join(scenario.dir_name()).join(filename);

        // Save current version to history
        if path.exists() {
            let existing = tokio::fs::read_to_string(&path).await?;
            let timestamp = Utc::now().to_rfc3339().replace(':', "-").replace('+', "Z");
            let history_path = self
                .base_dir
                .join(".history")
                .join(scenario.dir_name())
                .join(format!(
                    "{}.{}.md",
                    filename.trim_end_matches(".md"),
                    timestamp
                ));
            if let Some(parent) = history_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&history_path, existing).await?;
            self.prune_history(scenario, filename).await?;
        }

        // Write new content atomically
        Self::atomic_write(&path, &new_content).await?;
        Ok(())
    }

    /// Delete a memory file.
    pub async fn delete(&self, scenario: Scenario, filename: &str) -> Result<()> {
        let path = self.base_dir.join(scenario.dir_name()).join(filename);
        tokio::fs::remove_file(&path)
            .await
            .with_context(|| format!("Failed to delete memory file: {}", filename))
    }

    /// List all memory files in a scenario (excluding README.md and dotfiles).
    pub async fn list(&self, scenario: Scenario) -> Result<Vec<String>> {
        let dir = self.base_dir.join(scenario.dir_name());
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut entries = tokio::fs::read_dir(&dir).await?;
        let mut files = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") && name != "README.md" && !name.starts_with('.') {
                files.push(name);
            }
        }
        files.sort();
        Ok(files)
    }

    /// Check if a memory file exists.
    pub async fn exists(&self, scenario: Scenario, filename: &str) -> bool {
        let path = self.base_dir.join(scenario.dir_name()).join(filename);
        path.exists()
    }

    /// Prune history files to keep only MAX_HISTORY_VERSIONS most recent.
    async fn prune_history(&self, scenario: Scenario, filename: &str) -> Result<()> {
        let stem = filename.trim_end_matches(".md");
        let history_dir = self.base_dir.join(".history").join(scenario.dir_name());

        if !history_dir.exists() {
            return Ok(());
        }

        let mut entries = tokio::fs::read_dir(&history_dir).await?;
        let mut versions: Vec<(String, std::time::SystemTime)> = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(stem) && name.ends_with(".md") {
                if let Ok(metadata) = entry.metadata().await {
                    if let Ok(modified) = metadata.modified() {
                        versions.push((name, modified));
                    }
                }
            }
        }

        // Sort by modification time, newest first
        versions.sort_by(|a, b| b.1.cmp(&a.1));

        // Remove oldest versions beyond the limit
        for (name, _) in versions.into_iter().skip(MAX_HISTORY_VERSIONS) {
            let path = history_dir.join(&name);
            let _ = tokio::fs::remove_file(&path).await; // best-effort
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn temp_store() -> FileMemoryStore {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileMemoryStore::new(tmp.path().to_path_buf());
        store.init().await.unwrap();
        store
    }

    #[tokio::test]
    async fn test_init_creates_all_scenario_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileMemoryStore::new(tmp.path().to_path_buf());
        store.init().await.unwrap();
        for scenario in Scenario::all() {
            let dir = tmp.path().join(scenario.dir_name());
            assert!(dir.exists(), "Missing dir: {:?}", dir);
        }
        // History dirs too
        for scenario in Scenario::all() {
            let dir = tmp.path().join(".history").join(scenario.dir_name());
            assert!(dir.exists(), "Missing history dir: {:?}", dir);
        }
    }

    #[tokio::test]
    async fn test_create_and_read() {
        let store = temp_store().await;
        let tags = vec!["rust".to_string(), "async".to_string()];
        let filename = store
            .create(
                Scenario::Knowledge,
                "Test Memory",
                "concept",
                &tags,
                "Body content here",
            )
            .await
            .unwrap();

        let mem = store.read(Scenario::Knowledge, &filename).await.unwrap();
        assert_eq!(mem.metadata.title, "Test Memory");
        assert_eq!(mem.metadata.scenario, Scenario::Knowledge);
        assert_eq!(mem.metadata.tags, tags);
        assert!(mem.content.contains("Body content here"));
        assert!(mem.metadata.id.starts_with("mem_"));
    }

    #[tokio::test]
    async fn test_update_preserves_history() {
        let store = temp_store().await;
        let filename = store
            .create(Scenario::Decisions, "Original", "design", &[], "V1 content")
            .await
            .unwrap();

        // Read original, modify, update
        let original = store.read(Scenario::Decisions, &filename).await.unwrap();
        let updated = serialize_memory_file(&original.metadata, "V2 content");
        store
            .update(Scenario::Decisions, &filename, &updated)
            .await
            .unwrap();

        // Verify content changed
        let reloaded = store.read(Scenario::Decisions, &filename).await.unwrap();
        assert!(reloaded.content.contains("V2 content"));

        // Verify history file exists
        let history_dir = store.base_dir.join(".history").join("decisions");
        let entries: Vec<_> = std::fs::read_dir(&history_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1, "Should have 1 history version");
    }

    #[tokio::test]
    async fn test_delete_removes_file() {
        let store = temp_store().await;
        let filename = store
            .create(Scenario::Episodes, "To delete", "incident", &[], "content")
            .await
            .unwrap();

        store.delete(Scenario::Episodes, &filename).await.unwrap();
        assert!(!store.exists(Scenario::Episodes, &filename).await);
    }

    #[tokio::test]
    async fn test_list_returns_valid_files() {
        let store = temp_store().await;
        store
            .create(Scenario::Knowledge, "M1", "concept", &[], "c1")
            .await
            .unwrap();
        store
            .create(Scenario::Knowledge, "M2", "pattern", &[], "c2")
            .await
            .unwrap();

        let files = store.list(Scenario::Knowledge).await.unwrap();
        assert_eq!(files.len(), 2);
    }

    #[tokio::test]
    async fn test_list_excludes_index_and_dotfiles() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileMemoryStore::new(tmp.path().to_path_buf());
        store.init().await.unwrap();

        // Create a memory file
        store
            .create(Scenario::Knowledge, "Real", "concept", &[], "content")
            .await
            .unwrap();
        // Create README.md (should be excluded)
        tokio::fs::write(tmp.path().join("knowledge/README.md"), "readme")
            .await
            .unwrap();
        // Create .dotfile (should be excluded)
        tokio::fs::write(tmp.path().join("knowledge/.hidden.md"), "hidden")
            .await
            .unwrap();

        let files = store.list(Scenario::Knowledge).await.unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].starts_with("mem_"));
    }
}
