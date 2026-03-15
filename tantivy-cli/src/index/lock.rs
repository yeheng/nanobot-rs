//! Process-level index lock.
//!
//! Uses file locking to prevent multiple CLI processes from operating
//! on the same index simultaneously.

use std::fs::File;
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::{Error, Result};

/// Index-level exclusive lock.
///
/// This lock prevents multiple processes from accessing the same index
/// concurrently. The lock is automatically released when dropped (RAII).
pub struct IndexLock {
    _lock_file: File,
    lock_path: PathBuf,
}

impl IndexLock {
    /// Acquire an exclusive lock for the index (blocking wait).
    ///
    /// This will block until the lock is available.
    /// The lock is released automatically when the returned `IndexLock` is dropped.
    pub fn acquire(index_path: &Path) -> Result<Self> {
        let lock_path = index_path.join(".index.lock");

        // Ensure the directory exists
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                Error::LockError(format!(
                    "Failed to create index directory {:?}: {}",
                    parent, e
                ))
            })?;
        }

        let lock_file = File::create(&lock_path).map_err(|e| {
            Error::LockError(format!("Failed to create lock file {:?}: {}", lock_path, e))
        })?;

        lock_file
            .lock_exclusive()
            .map_err(|e| Error::LockError(format!("Failed to acquire lock: {}", e)))?;

        Ok(Self {
            _lock_file: lock_file,
            lock_path,
        })
    }
}

impl Drop for IndexLock {
    fn drop(&mut self) {
        let _ = self._lock_file.unlock();
        // Clean up lock file on release
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_lock_acquire_release() {
        let dir = tempdir().unwrap();
        let index_path = dir.path().join("test_index");

        // Should be able to acquire lock
        let lock = IndexLock::acquire(&index_path).unwrap();
        drop(lock);

        // Should be able to acquire again after release
        let lock2 = IndexLock::acquire(&index_path).unwrap();
        drop(lock2);
    }
}
