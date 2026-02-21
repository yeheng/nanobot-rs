//! Memory store trait and implementations

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tracing::debug;

/// A single memory entry.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    /// The storage key.
    pub key: String,

    /// The stored value.
    pub updated_at: DateTime<Utc>,
}

/// Structured query for memory operations.
#[derive(Debug, Clone, Default)]
pub struct MemoryQuery {
    /// Filter by key prefix.
    pub prefix: Option<String>,

    /// Maximum number of results.
    pub limit: Option<usize>,
}

/// Abstract storage interface for long-term memory.
///
/// Implementations must be safe to share across async tasks.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Read a value by key.
    async fn read(&self, key: &str) -> anyhow::Result<Option<String>>;

    /// Write a value by key.
    async fn write(&self, key: &str, value: &str) -> anyhow::Result<()>;

    /// Delete a value by key.
    async fn delete(&self, key: &str) -> anyhow::Result<bool>;

    /// Append to an existing value (useful for history/logs).
    async fn append(&self, key: &str, value: &str) -> anyhow::Result<()>;

    /// Query entries matching a filter.
    async fn query(&self, query: MemoryQuery) -> anyhow::Result<Vec<MemoryEntry>>;
}

// ──────────────────────────────────────────────
//  FileMemoryStore — file-based implementation
// ──────────────────────────────────────────────

/// File-based memory store that persists data under a workspace directory.
///
/// Compatible with the original `agent::memory::MemoryStore` file layout
/// (`memory/MEMORY.md`, `memory/HISTORY.md`).
///
/// Uses per-key locks (via a HashMap of mutex guards) instead of a global lock,
/// allowing concurrent operations on different keys.
pub struct FileMemoryStore {
    memory_dir: PathBuf,
    /// Per-key locks for write operations to prevent concurrent file corruption
    key_locks: Mutex<HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
}

impl FileMemoryStore {
    /// Create a new file-based memory store.
    pub fn new(workspace: PathBuf) -> Self {
        let memory_dir = workspace.join("memory");
        let _ = std::fs::create_dir_all(&memory_dir);
        Self {
            memory_dir,
            key_locks: Mutex::new(HashMap::new()),
        }
    }

    fn key_to_path(&self, key: &str) -> PathBuf {
        // Use the key directly as filename (sanitised)
        let safe_key = key.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
        self.memory_dir.join(safe_key)
    }

    /// Get or create a lock for a specific key
    fn get_key_lock(&self, key: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.key_locks.lock().unwrap();
        locks
            .entry(key.to_string())
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
}

#[async_trait]
impl MemoryStore for FileMemoryStore {
    async fn read(&self, key: &str) -> anyhow::Result<Option<String>> {
        let path = self.key_to_path(key);

        // Use spawn_blocking to avoid blocking the async runtime
        let content = tokio::task::spawn_blocking(move || {
            if path.exists() {
                std::fs::read_to_string(path).map(Some)
            } else {
                Ok(None)
            }
        })
        .await??;

        Ok(content)
    }

    async fn write(&self, key: &str, value: &str) -> anyhow::Result<()> {
        // Acquire per-key lock (not global lock)
        let lock = self.get_key_lock(key);
        let _guard = lock.lock().await;

        let path = self.key_to_path(key);
        let tmp_path = path.with_extension("tmp");
        let key_owned = key.to_string();
        let value_owned = value.to_string();

        tokio::task::spawn_blocking(move || {
            // Write to tmp file first
            std::fs::write(&tmp_path, value_owned)?;
            // Atomic rename
            std::fs::rename(&tmp_path, &path)?;
            debug!("Wrote memory key: {}", key_owned);
            Ok::<_, anyhow::Error>(())
        })
        .await??;

        Ok(())
    }

    async fn delete(&self, key: &str) -> anyhow::Result<bool> {
        // Acquire per-key lock (not global lock)
        let lock = self.get_key_lock(key);
        let _guard = lock.lock().await;

        let path = self.key_to_path(key);

        let deleted = tokio::task::spawn_blocking(move || {
            if path.exists() {
                std::fs::remove_file(path)?;
                Ok::<bool, anyhow::Error>(true)
            } else {
                Ok(false)
            }
        })
        .await??;

        Ok(deleted)
    }

    async fn append(&self, key: &str, value: &str) -> anyhow::Result<()> {
        // Acquire per-key lock (not global lock)
        // Note: We still need per-key lock to prevent interleaved appends
        // to the same file from different tasks
        let lock = self.get_key_lock(key);
        let _guard = lock.lock().await;

        let path = self.key_to_path(key);
        let key_owned = key.to_string();
        let value_owned = value.to_string();

        tokio::task::spawn_blocking(move || {
            use std::io::Write;
            // O_APPEND is used here for atomic append at the OS level
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)?
                .write_all(value_owned.as_bytes())?;
            debug!("Appended to memory key: {}", key_owned);
            Ok::<_, anyhow::Error>(())
        })
        .await??;

        Ok(())
    }

    async fn query(&self, query: MemoryQuery) -> anyhow::Result<Vec<MemoryEntry>> {
        let memory_dir = self.memory_dir.clone();

        tokio::task::spawn_blocking(move || {
            let mut entries = Vec::new();
            if let Ok(dir) = std::fs::read_dir(&memory_dir) {
                for entry in dir.flatten() {
                    let filename = entry.file_name().to_string_lossy().to_string();
                    if let Some(prefix) = &query.prefix {
                        if !filename.starts_with(prefix) {
                            continue;
                        }
                    }
                    let modified = entry
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .map(|t| DateTime::<Utc>::from(t))
                        .unwrap_or_else(Utc::now);
                    entries.push(MemoryEntry {
                        key: filename,
                        updated_at: modified,
                    });
                    if let Some(limit) = query.limit {
                        if entries.len() >= limit {
                            break;
                        }
                    }
                }
            }
            Ok(entries)
        })
        .await?
    }
}

// ──────────────────────────────────────────────
//  InMemoryStore — in-memory (testing)
// ──────────────────────────────────────────────

/// In-memory store for testing. Not persisted.
pub struct InMemoryStore {
    data: Mutex<HashMap<String, String>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            data: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MemoryStore for InMemoryStore {
    async fn read(&self, key: &str) -> anyhow::Result<Option<String>> {
        Ok(self.data.lock().unwrap().get(key).cloned())
    }

    async fn write(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.data
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    async fn delete(&self, key: &str) -> anyhow::Result<bool> {
        Ok(self.data.lock().unwrap().remove(key).is_some())
    }

    async fn append(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let mut data = self.data.lock().unwrap();
        let entry = data.entry(key.to_string()).or_default();
        entry.push_str(value);
        Ok(())
    }

    async fn query(&self, query: MemoryQuery) -> anyhow::Result<Vec<MemoryEntry>> {
        let data = self.data.lock().unwrap();
        let mut entries: Vec<MemoryEntry> = data
            .iter()
            .filter(|(k, _)| {
                if let Some(prefix) = &query.prefix {
                    k.starts_with(prefix)
                } else {
                    true
                }
            })
            .map(|(k, _)| MemoryEntry {
                key: k.clone(),
                updated_at: Utc::now(),
            })
            .collect();

        if let Some(limit) = query.limit {
            entries.truncate(limit);
        }

        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_in_memory_store_read_write() {
        let store = InMemoryStore::new();

        assert_eq!(store.read("key1").await.unwrap(), None);

        store.write("key1", "value1").await.unwrap();
        assert_eq!(store.read("key1").await.unwrap(), Some("value1".to_string()));
    }

    #[tokio::test]
    async fn test_in_memory_store_delete() {
        let store = InMemoryStore::new();

        store.write("key1", "value1").await.unwrap();
        assert!(store.delete("key1").await.unwrap());
        assert!(!store.delete("key1").await.unwrap());
        assert_eq!(store.read("key1").await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_in_memory_store_append() {
        let store = InMemoryStore::new();

        store.append("log", "line1\n").await.unwrap();
        store.append("log", "line2\n").await.unwrap();
        assert_eq!(
            store.read("log").await.unwrap(),
            Some("line1\nline2\n".to_string())
        );
    }

    #[tokio::test]
    async fn test_in_memory_store_query() {
        let store = InMemoryStore::new();

        store.write("mem_a", "1").await.unwrap();
        store.write("mem_b", "2").await.unwrap();
        store.write("other", "3").await.unwrap();

        let results = store
            .query(MemoryQuery {
                prefix: Some("mem_".to_string()),
                limit: None,
            })
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_in_memory_store_query_limit() {
        let store = InMemoryStore::new();

        store.write("a", "1").await.unwrap();
        store.write("b", "2").await.unwrap();
        store.write("c", "3").await.unwrap();

        let results = store
            .query(MemoryQuery {
                prefix: None,
                limit: Some(2),
            })
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_file_memory_store() {
        let dir = std::env::temp_dir().join(format!("nanobot_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let store = FileMemoryStore::new(dir.clone());

        store.write("MEMORY.md", "# Memory").await.unwrap();
        assert_eq!(
            store.read("MEMORY.md").await.unwrap(),
            Some("# Memory".to_string())
        );

        store
            .append("HISTORY.md", "[2025-01-01] event1\n")
            .await
            .unwrap();
        store
            .append("HISTORY.md", "[2025-01-02] event2\n")
            .await
            .unwrap();
        let history = store.read("HISTORY.md").await.unwrap().unwrap();
        assert!(history.contains("event1"));
        assert!(history.contains("event2"));

        assert!(store.delete("MEMORY.md").await.unwrap());
        assert_eq!(store.read("MEMORY.md").await.unwrap(), None);

        // Cleanup
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Test that concurrent writes to different keys don't block each other
    #[tokio::test]
    async fn test_file_memory_store_concurrent_different_keys() {
        let dir = std::env::temp_dir().join(format!("nanobot_test_concurrent_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let store = std::sync::Arc::new(FileMemoryStore::new(dir.clone()));

        // Spawn concurrent writes to different keys
        let mut handles = vec![];
        for i in 0..10 {
            let store = store.clone();
            let handle = tokio::spawn(async move {
                let key = format!("key_{}", i);
                store.write(&key, &format!("value_{}", i)).await.unwrap();
                let read = store.read(&key).await.unwrap();
                assert_eq!(read, Some(format!("value_{}", i)));
            });
            handles.push(handle);
        }

        // Wait for all to complete
        for handle in handles {
            handle.await.unwrap();
        }

        // Cleanup
        let _ = std::fs::remove_dir_all(dir);
    }
}
