//! Backup and restore operations.

use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::index::IndexManager;
use crate::Result;

/// Backup an index to a specified path.
pub fn backup_index(
    manager: &IndexManager,
    index_name: &str,
    backup_path: &Path,
) -> Result<BackupResult> {
    let stats = manager.get_stats(index_name)?;

    // Get index directory
    let index_dir = manager.index_path(index_name);

    // Create backup directory
    let backup_dir = backup_path.join(index_name);
    fs::create_dir_all(&backup_dir)?;

    // Copy all files
    copy_dir_all(&index_dir, &backup_dir)?;

    info!("Backed up index {} to {:?}", index_name, backup_dir);

    Ok(BackupResult {
        index_name: index_name.to_string(),
        backup_path: backup_dir,
        timestamp: Utc::now(),
        doc_count: stats.doc_count,
        size_bytes: stats.size_bytes,
    })
}

/// Restore an index from a backup.
pub fn restore_index(manager: &mut IndexManager, backup_path: &Path) -> Result<RestoreResult> {
    // Read metadata to get index name
    let metadata_path = backup_path.join("metadata.json");
    let metadata_json = fs::read_to_string(&metadata_path)?;
    let metadata: IndexMetadata = serde_json::from_str(&metadata_json)?;

    let index_name = metadata.schema.name.clone();

    // Drop existing index if it exists
    if manager.get_schema(&index_name)?.is_some() {
        manager.drop_index(&index_name)?;
    }

    // Copy backup to index directory
    let index_dir = manager.index_path(&index_name);
    fs::create_dir_all(&index_dir)?;
    copy_dir_all(backup_path, &index_dir)?;

    // Reload the index
    manager.load_indexes()?;

    info!("Restored index {} from {:?}", index_name, backup_path);

    Ok(RestoreResult {
        index_name,
        restore_path: backup_path.to_path_buf(),
        timestamp: Utc::now(),
    })
}

/// Copy directory recursively.
fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if ty.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

/// Result of backup operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupResult {
    /// Index name.
    pub index_name: String,
    /// Backup path.
    pub backup_path: std::path::PathBuf,
    /// Backup timestamp.
    pub timestamp: DateTime<Utc>,
    /// Document count at backup time.
    pub doc_count: u64,
    /// Size in bytes at backup time.
    pub size_bytes: u64,
}

/// Result of restore operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreResult {
    /// Index name.
    pub index_name: String,
    /// Restore source path.
    pub restore_path: std::path::PathBuf,
    /// Restore timestamp.
    pub timestamp: DateTime<Utc>,
}

/// Index metadata (for deserialization).
#[derive(Debug, Serialize, Deserialize)]
struct IndexMetadata {
    schema: crate::index::IndexSchema,
    config: crate::index::IndexConfig,
}
