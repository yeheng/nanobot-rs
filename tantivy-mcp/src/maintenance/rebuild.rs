//! Index rebuild operations.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::index::{Document, FieldDef, IndexManager};
use crate::Result;

/// Result of rebuild operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebuildResult {
    /// Index name.
    pub index_name: String,
    /// Documents reindexed.
    pub docs_reindexed: u64,
    /// Whether schema was changed.
    pub schema_changed: bool,
    /// Timestamp.
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Rebuild an index with optional new schema.
///
/// This operation:
/// 1. Reads all documents from the existing index (with read lock, then release)
/// 2. Drops the old index (with write lock on indexes map, then release)
/// 3. Creates a new index with the new schema (with write lock on indexes map, then release)
/// 4. Reindexes all documents (no global lock needed, only per-index lock)
///
/// IMPORTANT: This function does NOT require &mut IndexManager. All internal
/// locking is handled by the IndexManager methods themselves, which acquire
/// locks only when needed and release them immediately.
pub fn rebuild_index(
    manager: &IndexManager,
    index_name: &str,
    new_fields: Option<Vec<FieldDef>>,
    batch_size: usize,
) -> Result<RebuildResult> {
    info!("Rebuilding index: {}", index_name);

    // Phase 1: Get existing schema and all documents (read-only operations)
    // This only needs read access to the specific index, not global write lock
    let old_schema = manager
        .get_schema(index_name)?
        .ok_or_else(|| crate::Error::IndexNotFound(index_name.to_string()))?;

    // Use new schema or keep existing
    let schema_changed = new_fields.is_some();
    let fields = new_fields.unwrap_or_else(|| old_schema.fields.clone());

    // Get all documents from existing index (only needs read lock on index state)
    let docs = manager.list_documents(index_name, usize::MAX, 0)?;

    let docs_count = docs.len() as u64;
    info!("Found {} documents to reindex", docs_count);

    // Phase 2: Drop the old index (acquires write lock on indexes map briefly)
    manager.drop_index(index_name)?;
    info!("Dropped old index: {}", index_name);

    // Phase 3: Create new index with the schema (acquires write lock on indexes map briefly)
    manager.create_index(index_name, fields, None)?;
    info!("Created new index: {}", index_name);

    // Phase 4: Reindex documents in batches
    // These operations only need per-index locks, not global lock
    let mut indexed = 0u64;
    for chunk in docs.chunks(batch_size.max(1)) {
        for doc in chunk {
            manager.add_document(index_name, doc.clone())?;
            indexed += 1;
        }
        // Commit each batch
        manager.commit(index_name)?;
    }

    info!("Reindexed {} documents", indexed);

    Ok(RebuildResult {
        index_name: index_name.to_string(),
        docs_reindexed: indexed,
        schema_changed,
        timestamp: Utc::now(),
    })
}

/// Prepare for rebuild by extracting documents without holding global lock.
///
/// Returns (documents, fields, schema_changed) tuple.
/// This is useful when you want to minimize global lock time.
#[allow(dead_code)]
pub fn prepare_rebuild(
    manager: &IndexManager,
    index_name: &str,
    new_fields: Option<Vec<FieldDef>>,
) -> Result<(Vec<Document>, Vec<FieldDef>, bool)> {
    // Get existing schema
    let old_schema = manager
        .get_schema(index_name)?
        .ok_or_else(|| crate::Error::IndexNotFound(index_name.to_string()))?;

    // Use new schema or keep existing
    let schema_changed = new_fields.is_some();
    let fields = new_fields.unwrap_or_else(|| old_schema.fields.clone());

    // Get all documents
    let docs = manager.list_documents(index_name, usize::MAX, 0)?;

    Ok((docs, fields, schema_changed))
}

/// Execute the rebuild phases with minimal lock holding.
///
/// This function is designed to be called after prepare_rebuild.
/// All locking is handled internally by IndexManager methods.
#[allow(dead_code)]
pub fn execute_rebuild(
    manager: &IndexManager,
    index_name: &str,
    docs: Vec<Document>,
    fields: Vec<FieldDef>,
    schema_changed: bool,
    batch_size: usize,
) -> Result<RebuildResult> {
    let docs_count = docs.len() as u64;
    info!(
        "Rebuilding index {} with {} documents",
        index_name, docs_count
    );

    // Phase 1: Drop old index (acquires write lock on indexes map briefly)
    manager.drop_index(index_name)?;

    // Phase 2: Create new index (acquires write lock on indexes map briefly)
    manager.create_index(index_name, fields, None)?;

    // Phase 3: Reindex documents (only per-index locks needed)
    let mut indexed = 0u64;
    for chunk in docs.chunks(batch_size.max(1)) {
        for doc in chunk {
            manager.add_document(index_name, doc.clone())?;
            indexed += 1;
        }
        manager.commit(index_name)?;
    }

    info!("Reindexed {} documents", indexed);

    Ok(RebuildResult {
        index_name: index_name.to_string(),
        docs_reindexed: indexed,
        schema_changed,
        timestamp: Utc::now(),
    })
}
