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
    fn scan_disk_files(&self) -> anyhow::Result<Vec<(String, i64, std::path::PathBuf)>> {
        let wiki_root = self.page_store.wiki_root();
        if !wiki_root.exists() {
            return Ok(vec![]);
        }

        let mut files = Vec::new();
        Self::scan_dir_recursive(wiki_root, wiki_root, &mut files)?;
        Ok(files)
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

    /// Incremental sync: only process files newer than their SQLite record.
    async fn sync_changed(&self) -> Result<usize, ToolError> {
        let disk_files = self.scan_disk_files().map_err(|e| {
            ToolError::ExecutionError(format!("Failed to scan wiki directory: {}", e))
        })?;

        let mut synced = 0usize;
        for (rel_path, disk_mtime, full_path) in disk_files {
            let needs_update = match self.page_store.read(&rel_path).await {
                Ok(page) => page.file_mtime < disk_mtime,
                Err(_) => true, // Not in SQLite → must insert
            };

            if !needs_update {
                debug!("WikiRefresh: {} is up-to-date", rel_path);
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

            self.page_store.write(&page).await.map_err(|e| {
                ToolError::ExecutionError(format!("Failed to write {} to SQLite: {}", rel_path, e))
            })?;

            if let Err(e) = self.page_index.upsert(&page) {
                warn!(
                    "WikiRefresh: failed to upsert {} to Tantivy: {}",
                    rel_path, e
                );
            } else {
                debug!("WikiRefresh: upserted {} to Tantivy", rel_path);
            }

            synced += 1;
        }

        info!("WikiRefresh: synced {} changed pages", synced);
        Ok(synced)
    }

    /// Full rebuild: delete Tantivy index, re-import all files from disk.
    async fn full_rebuild(&self) -> Result<usize, ToolError> {
        let disk_files = self.scan_disk_files().map_err(|e| {
            ToolError::ExecutionError(format!("Failed to scan wiki directory: {}", e))
        })?;

        // First pass: write all pages to SQLite
        let mut pages = Vec::with_capacity(disk_files.len());
        for (rel_path, disk_mtime, full_path) in disk_files {
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

            self.page_store.write(&page).await.map_err(|e| {
                ToolError::ExecutionError(format!("Failed to write {} to SQLite: {}", rel_path, e))
            })?;

            pages.push(page);
        }

        // Second pass: rebuild Tantivy from all pages in SQLite
        self.page_index
            .rebuild(&self.page_store)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Tantivy rebuild failed: {}", e)))?;

        info!(
            "WikiRefresh: full rebuild complete with {} pages",
            pages.len()
        );
        Ok(pages.len())
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
