//! Filesystem utilities for atomic operations.
//!
//! Provides `atomic_write` for crash-safe file writes using temp-file + rename.

use anyhow::{Context, Result};
use std::path::Path;
use tracing::debug;

/// Write data to a file atomically using temp-file + rename.
///
/// Writes to a `.tmp` file first, syncs to disk, then renames to the
/// final path. On POSIX systems (Linux, macOS), `rename` is atomic,
/// so a crash will never leave a partially-written (corrupted) file.
///
/// # Arguments
/// * `path` - The final path for the file
/// * `data` - The data to write
///
/// # Example
///
/// ```ignore
/// use gasket_storage::fs::atomic_write;
/// use std::path::Path;
///
/// async fn example() -> anyhow::Result<()> {
///     atomic_write(Path::new("/tmp/myfile.txt"), "Hello, World!").await?;
///     Ok(())
/// }
/// ```
pub async fn atomic_write(path: &Path, data: impl AsRef<[u8]>) -> Result<()> {
    let tmp_path = path.with_extension("tmp");

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

    debug!(
        "Atomic write: {} ({} bytes)",
        path.display(),
        data.as_ref().len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_atomic_write_creates_file() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        atomic_write(&file_path, "Hello, World!").await.unwrap();

        assert!(file_path.exists());
        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "Hello, World!");
    }

    #[tokio::test]
    async fn test_atomic_write_overwrites_existing() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        // Write initial content
        tokio::fs::write(&file_path, "Old content").await.unwrap();

        // Atomic write should replace it
        atomic_write(&file_path, "New content").await.unwrap();

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "New content");
    }

    #[tokio::test]
    async fn test_atomic_write_bytes() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.bin");

        let data: Vec<u8> = vec![0x00, 0x01, 0x02, 0x03];
        atomic_write(&file_path, &data).await.unwrap();

        let content = tokio::fs::read(&file_path).await.unwrap();
        assert_eq!(content, data);
    }
}
