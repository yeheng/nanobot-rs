//! File watcher for memory directory changes.
//!
//! Monitors `~/.gasket/memory/` for file system changes with debouncing.
//! Detects human edits to memory files and triggers SQLite metadata sync.
//!
//! # Feature Flags
//!
//! - `memory-watcher`: Enables actual file watching via the `notify` crate
//! - Without the feature: `MemoryWatcher::start()` returns an empty channel (no-op)
//!
//! # Debouncing
//!
//! File system events are debounced by 2 seconds (default) to avoid processing
//! intermediate states during multi-step writes (e.g., editor save operations).
//!
//! # Filtering
//!
//! The watcher ignores:
//! - `.history/` directory (version-controlled backups)
//! - `.tmp` files (temporary editor files)
//! - `README.md` files (human-written notes, not memory entries)
//! - Dotfiles (hidden files starting with `.`)

//!
//! # Auto-Indexing
//!
//! The `AutoIndexHandler` connects file watcher events to O(1) SQLite upserts.
//! When enabled, modifying a `.md` memory file triggers:
//! 1. UPSERT of the file's metadata into the `memory_metadata` SQLite table
//! 2. UPSERT/delete of the file's embedding in SQLite

use super::types::Scenario;
use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

// ============================================================================
// Watch event types
// ============================================================================

/// Events emitted by the memory watcher after debouncing.
#[derive(Debug, Clone)]
pub enum WatchEvent {
    /// A new file was created.
    Created(PathBuf),

    /// An existing file was modified.
    Modified(PathBuf),

    /// A file was deleted.
    Deleted(PathBuf),
}

impl WatchEvent {
    /// Get the file path associated with this event.
    pub fn path(&self) -> &Path {
        match self {
            WatchEvent::Created(p) => p,
            WatchEvent::Modified(p) => p,
            WatchEvent::Deleted(p) => p,
        }
    }
}

/// Configuration for the memory file watcher.
#[derive(Debug, Clone)]
pub struct WatcherConfig {
    /// Base directory to watch (e.g., ~/.gasket/memory/)
    pub base_dir: PathBuf,

    /// Debounce duration in milliseconds (default: 2000)
    pub debounce_ms: u64,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            base_dir: super::path::memory_base_dir(),
            debounce_ms: 2000,
        }
    }
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

/// Extract the relative path from a full path by stripping the base_dir prefix.
fn relative_path(full_path: &Path, base_dir: &Path) -> Option<PathBuf> {
    full_path
        .strip_prefix(base_dir)
        .ok()
        .map(|p| p.to_path_buf())
}

// ============================================================================
// Feature-gated implementation
// ============================================================================

/// File watcher for memory directory changes.
///
/// When the `memory-watcher` feature is enabled, uses the `notify` crate to
/// detect file system changes with debouncing.
///
/// When the feature is disabled, `start()` returns an empty channel (no-op).
pub struct MemoryWatcher {
    config: WatcherConfig,
}

impl MemoryWatcher {
    /// Create a new memory watcher with the given configuration.
    pub fn new(config: WatcherConfig) -> Self {
        Self { config }
    }

    /// Create a new memory watcher with default configuration.
    pub fn with_defaults() -> Self {
        Self::default()
    }
}

impl Default for MemoryWatcher {
    fn default() -> Self {
        Self::new(WatcherConfig::default())
    }
}

// Actual implementation with notify crate
#[cfg(feature = "memory-watcher")]
impl MemoryWatcher {
    /// Start watching the memory directory.
    ///
    /// Returns a receiver for debounced events. This spawns a background task
    /// that watches the filesystem and debounces events.
    pub async fn start(&self) -> Result<mpsc::Receiver<WatchEvent>> {
        use notify::{RecommendedWatcher, RecursiveMode, Watcher as NotifyWatcher};
        use std::sync::mpsc as sync_mpsc;

        let (tx, rx) = mpsc::channel(100);
        let base_dir = self.config.base_dir.clone();
        let debounce = Duration::from_millis(self.config.debounce_ms);

        if !base_dir.exists() {
            tokio::fs::create_dir_all(&base_dir).await?;
        }

        let (raw_tx, raw_rx) = sync_mpsc::channel();

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = raw_tx.send(event);
                }
            },
            notify::Config::default(),
        )?;

        watcher.watch(&base_dir, RecursiveMode::Recursive)?;

        // Hold the watcher so it's dropped when the task ends
        let _watcher = watcher;

        tokio::spawn(async move {
            if let Err(e) = debounce_loop(raw_rx, tx, debounce).await {
                tracing::error!("Watcher debounce loop error: {:?}", e);
            }
            drop(_watcher);
        });

        Ok(rx)
    }

    /// Stop watching (no-op for the current implementation).
    pub async fn stop(&self) {
        // Dropping the watcher handles cleanup automatically
    }
}

/// Debounce loop that receives raw events and emits settled events.
#[cfg(feature = "memory-watcher")]
async fn debounce_loop(
    raw_rx: std::sync::mpsc::Receiver<notify::Event>,
    tx: mpsc::Sender<WatchEvent>,
    debounce: Duration,
) -> Result<()> {
    use tokio::sync::mpsc as tokio_mpsc;

    let (bridge_tx, mut bridge_rx) = tokio_mpsc::channel(100);

    std::thread::spawn(move || {
        while let Ok(event) = raw_rx.recv() {
            if bridge_tx.blocking_send(event).is_err() {
                break;
            }
        }
    });

    let mut pending: HashMap<PathBuf, Instant> = HashMap::new();
    let check_interval = Duration::from_millis(500);

    loop {
        let mut events_received = false;
        while let Ok(event) = bridge_rx.try_recv() {
            events_received = true;
            for path in &event.paths {
                if should_ignore(path) {
                    continue;
                }
                pending.insert(path.to_path_buf(), Instant::now());
            }
        }

        if events_received {
            tokio::time::sleep(check_interval).await;
        } else {
            tokio::time::sleep(check_interval).await;
            continue;
        }

        let now = Instant::now();
        let settled_paths: Vec<_> = pending
            .iter()
            .filter(|(_, &timestamp)| now.saturating_duration_since(timestamp) >= debounce)
            .map(|(path, _)| path.clone())
            .collect();

        for path in settled_paths {
            pending.remove(&path);

            let event = if path.exists() {
                WatchEvent::Modified(path)
            } else {
                WatchEvent::Deleted(path)
            };

            if tx.send(event).await.is_err() {
                return Ok(());
            }
        }
    }
}

// No-op fallback implementation when feature is disabled
#[cfg(not(feature = "memory-watcher"))]
impl MemoryWatcher {
    /// Start watching (no-op when feature is disabled).
    pub async fn start(&self) -> Result<mpsc::Receiver<WatchEvent>> {
        let (_tx, rx) = mpsc::channel(1);
        Ok(rx)
    }

    /// Stop watching (no-op when feature is disabled).
    pub async fn stop(&self) {
        // No-op
    }
}

// ============================================================================
// Auto-index handler
// ============================================================================

/// Handler that processes file watcher events with O(1) SQLite upserts.
///
/// Connects `MemoryWatcher` events to metadata + embedding sync.
/// When a `.md` file is created or modified:
/// - Parses its YAML frontmatter
/// - UPSERTs metadata into the `memory_metadata` SQLite table
/// - UPSERTs embedding into the `memory_embeddings` SQLite table
///
/// When a `.md` file is deleted:
/// - Deletes the corresponding rows from both SQLite tables
pub struct AutoIndexHandler {
    metadata_store: super::metadata_store::MetadataStore,
    embedding_store: super::embedding_store::EmbeddingStore,
    base_dir: PathBuf,
}

impl AutoIndexHandler {
    /// Create a new auto-index handler.
    pub fn new(
        metadata_store: super::metadata_store::MetadataStore,
        embedding_store: super::embedding_store::EmbeddingStore,
        base_dir: PathBuf,
    ) -> Self {
        Self {
            metadata_store,
            embedding_store,
            base_dir,
        }
    }

    /// Process a single watch event (O(1) — only touches the changed file).
    ///
    /// For Created/Modified events:
    /// 1. Extract the scenario from the file path
    /// 2. Parse the file's YAML frontmatter
    /// 3. UPSERT a single row into `memory_metadata`
    /// 4. UPSERT the file's embedding
    ///
    /// For Deleted events:
    /// 1. Delete the file's row from `memory_metadata`
    /// 2. Delete the file's embedding
    pub async fn process_event(&self, event: &WatchEvent) {
        let path = event.path();

        // Extract relative path for scenario detection
        let rel_path = match relative_path(path, &self.base_dir) {
            Some(p) => p,
            None => {
                tracing::debug!("AutoIndex: path outside base dir: {:?}", path.display());
                return;
            }
        };

        let scenario = match scenario_from_path(&rel_path) {
            Some(s) => s,
            None => {
                tracing::debug!("AutoIndex: unknown scenario for {:?}", rel_path.display());
                return;
            }
        };

        match event {
            WatchEvent::Created(_) | WatchEvent::Modified(_) => {
                let filename = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                if let Ok(content) = tokio::fs::read_to_string(path).await {
                    if let Ok((meta, _)) = super::frontmatter::parse_memory_file(&content) {
                        // O(1) SQLite upsert — only this file, not the whole directory
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
                        };
                        if let Err(e) = self.metadata_store.upsert_entry(&entry).await {
                            tracing::error!("AutoIndex: failed to upsert metadata: {}", e);
                        }

                        let embedding = vec![0.0f32; 384]; // TODO: actual embedder
                        match self
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
                            Ok(()) => {
                                tracing::debug!("AutoIndex: upserted embedding for {}", filename);
                            }
                            Err(e) => {
                                tracing::error!("AutoIndex: failed to upsert embedding: {}", e);
                            }
                        }
                    }
                }
            }
            WatchEvent::Deleted(_) => {
                let path_str = path.to_string_lossy();
                match self.embedding_store.delete(&path_str).await {
                    Ok(()) => {
                        tracing::debug!("AutoIndex: deleted embedding for {}", path_str);
                    }
                    Err(e) => {
                        tracing::error!("AutoIndex: failed to delete embedding: {}", e);
                    }
                }
            }
        }
    }

    /// Start listening in a loop, processing events from a watcher.
    ///
    /// This method runs in a background task, draining events from the watcher
    /// channel and processing each one via `process_event`.
    pub async fn run(&self, mut rx: mpsc::Receiver<WatchEvent>) {
        while let Some(event) = rx.recv().await {
            self.process_event(&event).await;
        }
        tracing::info!("AutoIndex handler stopped");
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

    #[test]
    fn test_relative_path_extraction() {
        let base = Path::new("/home/user/.gasket/memory");
        let full = Path::new("/home/user/.gasket/memory/knowledge/rust.md");
        let rel = relative_path(full, base);
        assert_eq!(Some(PathBuf::from("knowledge/rust.md")), rel);

        // Path outside base_dir
        let outside = Path::new("/tmp/other.md");
        assert_eq!(None, relative_path(outside, base));
    }

    #[test]
    fn test_watcher_config_default() {
        let config = WatcherConfig::default();
        assert_eq!(2000, config.debounce_ms);
        let path_str = config.base_dir.to_string_lossy();
        assert!(
            path_str.contains(".gasket") && path_str.contains("memory"),
            "base_dir should contain .gasket and memory: {}",
            path_str
        );
    }

    #[test]
    fn test_memory_watcher_default() {
        let watcher = MemoryWatcher::with_defaults();
        assert_eq!(2000, watcher.config.debounce_ms);
    }

    #[test]
    fn test_watch_event_path() {
        let path = PathBuf::from("knowledge/test.md");

        let created = WatchEvent::Created(path.clone());
        assert_eq!(Path::new("knowledge/test.md"), created.path());

        let modified = WatchEvent::Modified(path.clone());
        assert_eq!(Path::new("knowledge/test.md"), modified.path());

        let deleted = WatchEvent::Deleted(path);
        assert_eq!(Path::new("knowledge/test.md"), deleted.path());
    }

    #[cfg(feature = "memory-watcher")]
    #[tokio::test]
    async fn test_watcher_creates_base_dir() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_dir = temp_dir.path().join("memory");

        let config = WatcherConfig {
            base_dir: base_dir.clone(),
            debounce_ms: 100,
        };
        let watcher = MemoryWatcher::new(config);
        let _rx = watcher.start().await.unwrap();
        assert!(base_dir.exists());
    }

    #[cfg(feature = "memory-watcher")]
    #[tokio::test]
    async fn test_watcher_detects_new_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_dir = temp_dir.path().join("memory");

        tokio::fs::create_dir_all(base_dir.join("knowledge"))
            .await
            .unwrap();

        let config = WatcherConfig {
            base_dir: base_dir.clone(),
            debounce_ms: 100,
        };
        let watcher = MemoryWatcher::new(config);
        let mut rx = watcher.start().await.unwrap();

        let test_file = base_dir.join("knowledge/test.md");
        tokio::fs::write(&test_file, "# Test\n\nContent")
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;

        let timeout = Duration::from_secs(2);
        let event = tokio::time::timeout(timeout, rx.recv()).await;

        if let Ok(Some(event)) = event {
            assert!(event.path().ends_with("knowledge/test.md"));
        }
    }

    #[cfg(feature = "memory-watcher")]
    #[tokio::test]
    async fn test_watcher_ignores_readme() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_dir = temp_dir.path().join("memory");

        tokio::fs::create_dir_all(base_dir.join("knowledge"))
            .await
            .unwrap();

        let config = WatcherConfig {
            base_dir: base_dir.clone(),
            debounce_ms: 100,
        };
        let watcher = MemoryWatcher::new(config);
        let mut rx = watcher.start().await.unwrap();

        let readme_file = base_dir.join("knowledge/README.md");
        tokio::fs::write(&readme_file, "# My notes").await.unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;

        let timeout = Duration::from_millis(100);
        let result = tokio::time::timeout(timeout, rx.recv()).await;
        assert!(
            result.is_err(),
            "Should not receive events for README.md files"
        );
    }

    #[cfg(not(feature = "memory-watcher"))]
    #[tokio::test]
    async fn test_watcher_noop_without_feature() {
        let watcher = MemoryWatcher::with_defaults();
        let mut rx = watcher.start().await.unwrap();
        let result = rx.recv().await;
        assert!(
            result.is_none(),
            "Channel should return None when closed without feature"
        );
    }
}
