//! Index compaction.

use crate::index::IndexManager;
use crate::Result;

/// Compact an index by merging segments and removing deleted documents.
pub fn compact_index(manager: &IndexManager, index_name: &str) -> Result<CompactionResult> {
    let stats_before = manager.get_stats(index_name)?;

    manager.compact(index_name)?;

    let stats_after = manager.get_stats(index_name)?;

    Ok(CompactionResult {
        index_name: index_name.to_string(),
        segments_before: stats_before.segment_count,
        segments_after: stats_after.segment_count,
        deleted_before: stats_before.deleted_count,
        deleted_after: stats_after.deleted_count,
        bytes_saved: stats_before
            .size_bytes
            .saturating_sub(stats_after.size_bytes),
    })
}

/// Result of compaction operation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompactionResult {
    /// Index name.
    pub index_name: String,
    /// Number of segments before compaction.
    pub segments_before: usize,
    /// Number of segments after compaction.
    pub segments_after: usize,
    /// Deleted documents before compaction.
    pub deleted_before: u64,
    /// Deleted documents after compaction.
    pub deleted_after: u64,
    /// Bytes saved by compaction.
    pub bytes_saved: u64,
}
