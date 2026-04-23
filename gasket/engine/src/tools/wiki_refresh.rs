//! Wiki refresh tool — syncs Markdown files → SQLite → Tantivy.
//!
//! Core principle: Markdown files are the SSOT. SQLite and Tantivy are
//! derived projections that must be kept in sync.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use tracing::{debug, info, instrument, warn};

use super::{Tool, ToolContext, ToolError, ToolResult};
use crate::wiki::{PageFilter, PageIndex, PageStore, PageType, WikiPage};

/// Tool for refreshing wiki page index and syncing from disk.
pub struct WikiRefreshTool {
    page_store: Arc<PageStore>,
    page_index: Arc<PageIndex>,
}

impl WikiRefreshTool {
    pub fn new(page_store: Arc<PageStore>, page_index: Arc<PageIndex>) -> Self {
        Self {
            page_store,
            page_index,
        }
    }

    /// Recursively scan `wiki_root` for `.md` files and return
    /// (relative_path, disk_mtime, full_path) tuples.
    ///
    /// Runs the synchronous filesystem walk inside `spawn_blocking` to avoid
    /// blocking the Tokio runtime during recursive directory traversal.
    async fn scan_disk_files(&self) -> anyhow::Result<Vec<(String, i64, std::path::PathBuf)>> {
        let wiki_root = self.page_store.wiki_root().to_path_buf();
        if !wiki_root.exists() {
            return Ok(vec![]);
        }

        tokio::task::spawn_blocking(move || {
            let mut files = Vec::new();
            Self::scan_dir_recursive(&wiki_root, &wiki_root, &mut files)?;
            Ok(files)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Wiki scan task panicked: {}", e))?
    }

    fn scan_dir_recursive(
        root: &Path,
        dir: &Path,
        out: &mut Vec<(String, i64, std::path::PathBuf)>,
    ) -> anyhow::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                Self::scan_dir_recursive(root, &path, out)?;
            } else if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("md") {
                let rel = path.strip_prefix(root)?;
                let rel_str = {
                    let s = rel.to_string_lossy();
                    s.strip_suffix(".md").unwrap_or(&s).to_string()
                };

                let mtime = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);

                out.push((rel_str, mtime, path));
            }
        }
        Ok(())
    }

    /// Sync changed files and delete stale DB records.
    ///
    /// Uses mtime comparison to skip unmodified files. Deletes SQLite rows
    /// whose corresponding Markdown file has been removed on disk.
    async fn sync_changed(&self) -> Result<usize, ToolError> {
        let disk_files = self.scan_disk_files().await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to scan wiki directory: {}", e))
        })?;

        let disk_paths: std::collections::HashSet<String> =
            disk_files.iter().map(|(p, _, _)| p.clone()).collect();

        // Remove stale DB records (files deleted on disk).
        let db_pages = self
            .page_store
            .list(PageFilter::default())
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Failed to list DB pages: {}", e)))?;
        for page in db_pages {
            if !disk_paths.contains(&page.path) {
                if let Err(e) = self.page_store.delete(&page.path).await {
                    warn!(
                        "WikiRefresh: failed to delete stale DB record {}: {}",
                        page.path, e
                    );
                } else {
                    debug!("WikiRefresh: removed stale DB record {}", page.path);
                }
            }
        }

        let mut synced = 0usize;
        let mut max_seq = 0u64;
        for (rel_path, disk_mtime, full_path) in disk_files {
            // Lazy mtime check: skip if DB already has the same mtime.
            let needs_sync = match self.page_store.db().get(&rel_path).await {
                Ok(Some(row)) => row.file_mtime != disk_mtime,
                _ => true,
            };

            if !needs_sync {
                debug!("WikiRefresh: {} is up to date, skipping", rel_path);
                continue;
            }

            let markdown = tokio::fs::read_to_string(&full_path).await.map_err(|e| {
                ToolError::ExecutionError(format!("Failed to read {}: {}", full_path.display(), e))
            })?;

            let mut page = WikiPage::from_markdown(rel_path.clone(), &markdown).map_err(|e| {
                ToolError::ExecutionError(format!(
                    "Failed to parse markdown for {}: {}",
                    rel_path, e
                ))
            })?;
            page.file_mtime = disk_mtime;

            // Update SQLite index only (disk is already SSOT).
            match self.page_store.index_page(&page).await {
                Ok(seq) => max_seq = max_seq.max(seq),
                Err(e) => {
                    warn!("WikiRefresh: failed to index {} in SQLite: {}", rel_path, e);
                    continue;
                }
            }

            if let Err(e) = self.page_index.upsert(&page).await {
                warn!(
                    "WikiRefresh: failed to upsert {} to Tantivy: {}",
                    rel_path, e
                );
            } else {
                debug!("WikiRefresh: upserted {} to Tantivy", rel_path);
            }

            synced += 1;
        }

        // Update index watermark after successful batch.
        if max_seq > 0 {
            if let Err(e) = self.page_store.update_indexed_sequence(max_seq).await {
                warn!("WikiRefresh: failed to update index watermark: {}", e);
            }
        }

        info!("WikiRefresh: synced {} changed pages", synced);
        Ok(synced)
    }

    /// Full rebuild: sync SQLite from disk (removes stale records), then rebuild Tantivy.
    async fn full_rebuild(&self) -> Result<usize, ToolError> {
        let synced = self.page_store.sync_db_from_disk().await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to sync DB from disk: {}", e))
        })?;

        self.page_index
            .rebuild(&self.page_store)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Tantivy rebuild failed: {}", e)))?;

        let max_seq = self.page_store.db().max_sync_sequence().await.unwrap_or(0);
        if max_seq > 0 {
            if let Err(e) = self.page_store.update_indexed_sequence(max_seq).await {
                warn!(
                    "WikiRefresh: failed to update index watermark after rebuild: {}",
                    e
                );
            }
        }

        info!("WikiRefresh: full rebuild complete with {} pages", synced);
        Ok(synced)
    }

    /// Gather statistics.
    async fn stats(&self) -> Result<String, ToolError> {
        let all_pages = self
            .page_store
            .list(PageFilter::default())
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Failed to get wiki stats: {}", e)))?;

        let total = all_pages.len();
        let topics = all_pages
            .iter()
            .filter(|p| matches!(p.page_type, PageType::Topic))
            .count();
        let entities = all_pages
            .iter()
            .filter(|p| matches!(p.page_type, PageType::Entity))
            .count();
        let sources = all_pages
            .iter()
            .filter(|p| matches!(p.page_type, PageType::Source))
            .count();
        let sops = all_pages
            .iter()
            .filter(|p| matches!(p.page_type, PageType::Sop))
            .count();
        let index_docs = self.page_index.doc_count();

        Ok(format!(
            "📊 Wiki Statistics\n\nTotal pages: {}\nIndex docs: {}\n\nBy type:\n  📚 Topics: {}\n  👥 Entities: {}\n  📄 Sources: {}\n  📋 SOPs: {}",
            total, index_docs, topics, entities, sources, sops
        ))
    }
}

#[async_trait]
impl Tool for WikiRefreshTool {
    fn name(&self) -> &str {
        "wiki_refresh"
    }

    fn description(&self) -> &str {
        "Refresh and reindex wiki pages from Markdown files. Actions: 'sync' imports changed files only; 'reindex' does full rebuild of SQLite + Tantivy; 'stats' shows wiki statistics."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["sync", "reindex", "stats"],
                    "description": "Action to perform: 'sync' imports only changed Markdown files; 'reindex' rebuilds SQLite and Tantivy from all files; 'stats' shows current wiki statistics"
                }
            },
            "required": ["action"]
        })
    }

    #[instrument(name = "tool.wiki_refresh", skip_all)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        struct Args {
            action: String,
        }

        let args: Args =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        match args.action.as_str() {
            "sync" => {
                let count = self.sync_changed().await?;
                Ok(format!(
                    "✓ Wiki sync complete\n\nSynced {} changed pages.",
                    count
                ))
            }
            "reindex" => {
                let count = self.full_rebuild().await?;
                Ok(format!(
                    "✓ Wiki reindex complete\n\nReindexed {} pages from Markdown files.",
                    count
                ))
            }
            "stats" => self.stats().await,
            _ => Err(ToolError::InvalidArguments(format!(
                "Unknown action: '{}'. Valid actions are: 'sync', 'reindex', 'stats'",
                args.action
            ))),
        }
    }
}
