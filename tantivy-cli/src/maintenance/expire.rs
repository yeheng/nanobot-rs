//! Document expiration.

use chrono::Utc;
use tracing::info;

use crate::index::IndexManager;
use crate::Result;

/// Expire documents that have passed their TTL.
pub fn expire_documents(manager: &mut IndexManager, index_name: &str) -> Result<ExpirationResult> {
    // For now, we'll use a simple approach - search for expired documents
    // and delete them one by one. A more efficient approach would be to
    // use a range query on the _expires_at field.

    info!("Running expiration for index {}", index_name);

    // Get current doc count
    let stats_before = manager.get_stats(index_name)?;
    let doc_count_before = stats_before.doc_count;

    // Search for documents with _expires_at < now
    // This requires a custom query that we'll implement later
    // For now, return a placeholder result

    manager.commit(index_name)?;

    let stats_after = manager.get_stats(index_name)?;

    Ok(ExpirationResult {
        index_name: index_name.to_string(),
        expired_count: doc_count_before.saturating_sub(stats_after.doc_count),
        checked_at: Utc::now(),
    })
}

/// Result of expiration operation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExpirationResult {
    /// Index name.
    pub index_name: String,
    /// Number of expired documents removed.
    pub expired_count: u64,
    /// When the check was performed.
    pub checked_at: chrono::DateTime<chrono::Utc>,
}
