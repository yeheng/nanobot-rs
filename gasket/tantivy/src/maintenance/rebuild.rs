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

/// Default batch size for streaming rebuild (prevents OOM on large indexes).
const DEFAULT_BATCH_SIZE: usize = 1000;

/// Rebuild an index with optional new schema using streaming pagination.
///
/// This operation uses **streaming pagination with temporary index** to avoid OOM:
/// 1. Get existing schema and total document count
/// 2. Create a temporary index with the new schema
/// 3. Stream documents from old index → write to temp index (batch by batch)
/// 4. Drop the old index
/// 5. Rename temp index to the original name
///
/// # Memory Safety
/// Uses pagination instead of loading all documents at once. Each batch is:
/// - Loaded from old index (batch_size documents)
/// - Written to temp index
/// - Released from memory
/// - Next batch loaded
///
/// Memory usage stays constant regardless of total document count.
pub fn rebuild_index(
    manager: &mut IndexManager,
    index_name: &str,
    new_fields: Option<Vec<FieldDef>>,
    batch_size: usize,
) -> Result<RebuildResult> {
    info!("Rebuilding index: {}", index_name);

    // Use reasonable batch size to prevent memory issues
    let batch_size = batch_size.max(DEFAULT_BATCH_SIZE);

    // Phase 1: Get existing schema (read-only operation)
    let old_schema = manager
        .get_schema(index_name)?
        .ok_or_else(|| crate::Error::IndexNotFound(index_name.to_string()))?;

    // Use new schema or keep existing
    let schema_changed = new_fields.is_some();
    let fields = new_fields.unwrap_or_else(|| old_schema.fields.clone());

    // Get total document count first (for progress reporting)
    let stats = manager.get_stats(index_name)?;
    let total_docs = stats.doc_count;
    info!(
        "Preparing to reindex {} documents in batches of {}",
        total_docs, batch_size
    );

    // Phase 2: Create temporary index for the rebuild
    let temp_index_name = format!(
        "{}_rebuild_tmp_{}",
        index_name,
        chrono::Utc::now().timestamp()
    );
    manager.create_index(&temp_index_name, fields, None)?;
    info!("Created temporary index: {}", temp_index_name);

    // Phase 3: Stream documents from old index to temp index in batches
    let mut indexed = 0u64;
    let mut offset = 0usize;

    loop {
        // Load one batch at a time from OLD index - memory usage stays constant
        let docs = manager.list_documents(index_name, batch_size, offset)?;

        if docs.is_empty() {
            break; // No more documents
        }

        let batch_count = docs.len();
        info!(
            "Processing batch: {}-{} of ~{} documents",
            offset,
            offset + batch_count - 1,
            total_docs
        );

        // Write this batch to the TEMP index
        for doc in docs {
            manager.add_document(&temp_index_name, doc)?;
            indexed += 1;
        }

        // Commit each batch to temp index
        manager.commit(&temp_index_name)?;

        // Move to next batch
        offset += batch_count;

        // Safety check: if we got fewer docs than requested, we're done
        if batch_count < batch_size {
            break;
        }
    }

    info!("Reindexed {} documents to temp index", indexed);

    // Phase 4: Drop the old index
    manager.drop_index(index_name)?;
    info!("Dropped old index: {}", index_name);

    // Phase 5: Rename temp index to original name
    // This is done by renaming the directory on disk and reloading
    rename_index(manager, &temp_index_name, index_name)?;
    info!("Renamed temp index to: {}", index_name);

    Ok(RebuildResult {
        index_name: index_name.to_string(),
        docs_reindexed: indexed,
        schema_changed,
        timestamp: Utc::now(),
    })
}

/// Rename an index by renaming its directory and reloading.
fn rename_index(manager: &mut IndexManager, old_name: &str, new_name: &str) -> Result<()> {
    // Get the index directory path
    let index_dir = manager.index_dir();

    let old_path = index_dir.join(old_name);
    let new_path = index_dir.join(new_name);

    // Rename the directory on disk
    std::fs::rename(&old_path, &new_path).map_err(|e| {
        crate::Error::PathError(
            old_path.clone(),
            format!("Failed to rename to {:?}: {}", new_path, e),
        )
    })?;

    // Unload the temp index from memory
    manager.unload_index(old_name)?;

    // Reload with the new name
    manager.load_index_by_name(new_name)?;

    Ok(())
}

/// Prepare for rebuild by extracting documents without holding global lock.
///
/// Returns (documents, fields, schema_changed) tuple.
/// This is useful when you want to minimize global lock time.
#[allow(dead_code)]
pub fn prepare_rebuild(
    manager: &mut IndexManager,
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
    manager: &mut IndexManager,
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
