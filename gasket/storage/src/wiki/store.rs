use anyhow::Result;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;
use tokio::fs;

use crate::fs::atomic_write;
use crate::wiki::types::{PageFilter, PageSummary, PageType, WikiPage};

use super::lifecycle::{DecayReport, FrequencyManager};
use super::page_store::WikiPageStore;
use super::types::Frequency;

/// PageStore: CRUD operations on wiki pages with a **two-layer SSOT contract**.
///
/// - **Disk markdown files are the SSOT for content** (title, type, category,
///   tags, summary, body). Whatever is in `wiki_root/<path>.md` is the truth.
/// - **SQLite is the SSOT for runtime/index state** (access_count, frequency,
///   last_accessed, created/updated timestamps, source_count, confidence).
///   It is also a derived query projection — `list`/`read_many`/`read_summaries`
///   serve their result directly out of SQLite without touching disk.
///
/// `read(path)` enforces the contract: it pulls the markdown off disk first,
/// then overlays the runtime fields from the DB row. This guarantees that
/// out-of-band edits (e.g. `vim wiki_root/topics/foo.md`) are visible to
/// callers, while preserving the runtime stats that only the DB tracks.
///
/// `write(page)` writes disk first, then upserts the DB index — the order
/// matters: if the process crashes between disk-write and DB-upsert, the next
/// `sync_db_from_disk()` reconstructs the missing DB row from disk.
#[derive(Clone)]
pub struct PageStore {
    db: WikiPageStore,
    wiki_root: PathBuf,
    wiki_changed_tx: Option<tokio::sync::mpsc::Sender<String>>,
}

impl PageStore {
    pub fn new(pool: sqlx::SqlitePool, wiki_root: PathBuf) -> Self {
        Self {
            db: WikiPageStore::new(pool),
            wiki_root,
            wiki_changed_tx: None,
        }
    }

    /// Attach a channel for publishing wiki-changed notifications.
    /// When set, `write` and `delete` will send the affected path
    /// over this channel instead of touching the global broker.
    pub fn with_wiki_changed_tx(mut self, tx: tokio::sync::mpsc::Sender<String>) -> Self {
        self.wiki_changed_tx = Some(tx);
        self
    }

    /// Get the wiki root directory.
    pub fn wiki_root(&self) -> &PathBuf {
        &self.wiki_root
    }

    /// Run frequency decay batch on all stale pages.
    pub async fn run_decay_batch(&self) -> Result<DecayReport> {
        FrequencyManager::run_decay_batch(&self.db).await
    }

    /// Get metadata for a page by path (lightweight, no content).
    pub async fn get_metadata(&self, path: &str) -> Result<Option<PageSummary>> {
        match self.db.get(path).await? {
            Some(row) => Ok(Some(Self::row_to_summary(&row, row.content.len() as u64))),
            None => Ok(None),
        }
    }

    /// Ensure wiki directory structure exists.
    pub async fn init_dirs(&self) -> Result<()> {
        for dir in &[
            "entities/people",
            "entities/projects",
            "entities/concepts",
            "topics",
            "sources",
            "sops",
        ] {
            fs::create_dir_all(self.wiki_root.join(dir)).await?;
        }
        Ok(())
    }

    /// Read a page. Disk is SSOT for content; DB overlays runtime state.
    ///
    /// Resolution order:
    /// 1. Read `wiki_root/<path>.md` from disk → parse frontmatter + body.
    ///    Defaults for runtime fields (`access_count = 0`, `frequency = default`,
    ///    `created/updated = now`) are filled in by `from_markdown`.
    /// 2. If a matching DB row exists, overlay the runtime fields from it
    ///    (access_count, frequency, last_accessed, created, updated,
    ///    source_count, confidence). Content fields stay disk-fresh.
    /// 3. If disk file is missing, fall back to DB and surface a debug log —
    ///    that is a damaged-index state which `sync_db_from_disk` will fix.
    pub async fn read(&self, path: &str) -> Result<WikiPage> {
        let disk_path = self.wiki_root.join(format!("{}.md", path));
        match fs::read_to_string(&disk_path).await {
            Ok(markdown) => {
                let mut page = WikiPage::from_markdown(path.to_string(), &markdown)?;
                page.file_mtime = Self::file_mtime(&disk_path).await.unwrap_or(0);
                if let Some(row) = self.db.get(path).await? {
                    Self::overlay_runtime_state(&mut page, &row);
                }
                Ok(page)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(row) = self.db.get(path).await? {
                    tracing::debug!(
                        "PageStore::read('{}'): disk file missing, returning stale DB row \
                         (run sync_db_from_disk to repair)",
                        path
                    );
                    return Ok(Self::row_to_page(row));
                }
                Err(e.into())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Batch-read full pages from SQLite.
    pub async fn read_many(&self, paths: &[String]) -> Result<Vec<WikiPage>> {
        let rows = self.db.get_many(paths).await?;
        Ok(rows.into_iter().map(|row| Self::row_to_page(row)).collect())
    }

    /// Write page: disk is SSOT. Atomic write to disk first, then update SQLite.
    pub async fn write(&self, page: &WikiPage) -> Result<()> {
        self.sync_to_disk(page).await?;
        let disk_path = self.wiki_root.join(format!("{}.md", page.path));
        let mtime = Self::file_mtime(&disk_path).await.unwrap_or(0);
        self.upsert_db(page, mtime).await?;
        self.notify_wiki_changed(&page.path).await;
        Ok(())
    }

    /// Update SQLite index for a page that is already on disk.
    pub async fn index_page(&self, page: &WikiPage) -> Result<()> {
        let disk_path = self.wiki_root.join(format!("{}.md", page.path));
        let mtime = Self::file_mtime(&disk_path).await.unwrap_or(0);
        self.upsert_db(page, mtime).await
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        let disk_path = self.wiki_root.join(format!("{}.md", path));
        let _ = fs::remove_file(&disk_path).await;
        self.db.delete(path).await?;
        self.notify_wiki_changed(path).await;
        Ok(())
    }

    pub async fn exists(&self, path: &str) -> Result<bool> {
        self.db.exists(path).await
    }

    pub async fn list(&self, filter: PageFilter) -> Result<Vec<PageSummary>> {
        let rows = match &filter.page_type {
            Some(pt) => self.db.list_by_type(pt.as_str()).await?,
            None => self.db.list_all().await?,
        };
        Ok(rows
            .iter()
            .map(|r| Self::row_to_summary(r, r.content.len() as u64))
            .collect())
    }

    /// Batch-load lightweight page summaries for a set of paths.
    pub async fn read_summaries(&self, paths: &[String]) -> Result<Vec<PageSummary>> {
        let rows = self.db.get_summaries_by_paths(paths).await?;
        Ok(rows
            .into_iter()
            .map(|r| Self::summary_row_to_summary(r))
            .collect())
    }

    /// Sync page to disk as markdown using atomic write (crash-safe).
    pub async fn sync_to_disk(&self, page: &WikiPage) -> Result<()> {
        let path = self.wiki_root.join(format!("{}.md", page.path));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        atomic_write(&path, page.to_markdown()).await?;
        Ok(())
    }

    /// Sync SQLite index from disk. Upserts every `.md` file and removes DB
    /// records for files that no longer exist on disk.
    pub async fn sync_db_from_disk(&self) -> Result<usize> {
        let wiki_root = self.wiki_root.clone();
        let disk_paths = tokio::task::spawn_blocking(move || {
            let mut paths = std::collections::HashSet::new();
            Self::walk_disk_sync(&wiki_root, &wiki_root, &mut paths)?;
            Ok::<_, anyhow::Error>(paths)
        })
        .await
        .map_err(|e| anyhow::anyhow!("disk walk panicked: {}", e))??;

        // Remove stale DB records.
        let db_rows = self.db.list_all().await?;
        for row in &db_rows {
            if !disk_paths.contains(&row.path) {
                self.db.delete(&row.path).await?;
            }
        }

        // Re-index all files present on disk.
        let mut synced = 0usize;
        for path in &disk_paths {
            let full_path = self.wiki_root.join(format!("{}.md", path));
            let mtime = Self::file_mtime(&full_path).await.unwrap_or(0);
            let markdown = fs::read_to_string(&full_path).await?;
            let mut page = WikiPage::from_markdown(path.clone(), &markdown)?;
            page.file_mtime = mtime;

            // Preserve machine runtime state from existing DB record if any.
            if let Some(old) = self.db.get(path).await? {
                page.frequency = Frequency::from_str_lossy(&old.frequency);
                page.access_count = old.access_count as u64;
                page.last_accessed = old
                    .last_accessed
                    .as_deref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&chrono::Utc));
            }

            self.upsert_db(&page, mtime).await?;
            synced += 1;
        }

        Ok(synced)
    }

    // -- private helpers --

    fn parse_tags(tags: Option<&str>) -> Vec<String> {
        tags.and_then(|t| serde_json::from_str(t).ok()).unwrap_or_default()
    }

    fn parse_rfc3339(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_default()
    }

    fn parse_optional_rfc3339(s: Option<&str>) -> Option<chrono::DateTime<chrono::Utc>> {
        s.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc))
    }

    fn row_to_page(row: super::page_store::PageRow) -> WikiPage {
        WikiPage {
            path: row.path,
            title: row.title,
            page_type: row.page_type.parse().unwrap_or(PageType::Topic),
            category: row.category,
            tags: Self::parse_tags(row.tags.as_deref()),
            summary: row.summary,
            content: row.content,
            created: Self::parse_rfc3339(&row.created),
            updated: Self::parse_rfc3339(&row.updated),
            source_count: row.source_count as u32,
            confidence: row.confidence,
            frequency: Frequency::from_str_lossy(&row.frequency),
            access_count: row.access_count as u64,
            last_accessed: Self::parse_optional_rfc3339(row.last_accessed.as_deref()),
            file_mtime: row.file_mtime,
        }
    }

    /// Overlay DB runtime state onto a disk-loaded `WikiPage`.
    ///
    /// Disk supplies content fields (title/type/category/tags/summary/content);
    /// DB supplies the runtime/index fields that cannot live in the markdown
    /// frontmatter (timestamps, access stats, frequency, confidence).
    fn overlay_runtime_state(page: &mut WikiPage, row: &super::page_store::PageRow) {
        page.created = Self::parse_rfc3339(&row.created);
        page.updated = Self::parse_rfc3339(&row.updated);
        page.source_count = row.source_count as u32;
        page.confidence = row.confidence;
        page.frequency = Frequency::from_str_lossy(&row.frequency);
        page.access_count = row.access_count as u64;
        page.last_accessed = Self::parse_optional_rfc3339(row.last_accessed.as_deref());
    }

    fn row_to_summary(row: &super::page_store::PageRow, content_length: u64) -> PageSummary {
        PageSummary {
            path: row.path.clone(),
            title: row.title.clone(),
            page_type: row.page_type.parse().unwrap_or(PageType::Topic),
            category: row.category.clone(),
            tags: Self::parse_tags(row.tags.as_deref()),
            updated: Self::parse_rfc3339(&row.updated),
            confidence: row.confidence,
            frequency: Frequency::from_str_lossy(&row.frequency),
            access_count: row.access_count as u64,
            last_accessed: Self::parse_optional_rfc3339(row.last_accessed.as_deref()),
            summary: row.summary.clone(),
            content_length,
            file_mtime: row.file_mtime,
        }
    }

    fn summary_row_to_summary(row: super::page_store::PageSummaryRow) -> PageSummary {
        PageSummary {
            path: row.path,
            title: row.title,
            page_type: row.page_type.parse().unwrap_or(PageType::Topic),
            category: row.category,
            tags: Self::parse_tags(row.tags.as_deref()),
            updated: Self::parse_rfc3339(&row.updated),
            confidence: row.confidence,
            frequency: Frequency::from_str_lossy(&row.frequency),
            access_count: row.access_count as u64,
            last_accessed: Self::parse_optional_rfc3339(row.last_accessed.as_deref()),
            summary: row.summary,
            content_length: row.content_length as u64,
            file_mtime: row.file_mtime,
        }
    }

    async fn upsert_db(&self, page: &WikiPage, file_mtime: i64) -> Result<()> {
        let tags_str = serde_json::to_string(&page.tags)?;
        let checksum = Some(format!("{}", page.content.len()));
        self.db
            .upsert(&super::page_store::WikiPageInput {
                path: &page.path,
                title: &page.title,
                page_type: page.page_type.as_str(),
                category: page.category.as_deref(),
                tags: &tags_str,
                summary: page.summary.as_deref(),
                content: &page.content,
                source_count: page.source_count,
                confidence: page.confidence,
                checksum: checksum.as_deref(),
                frequency: page.frequency,
                access_count: page.access_count,
                last_accessed: page.last_accessed.map(|dt| dt.to_rfc3339()),
                file_mtime,
            })
            .await?;
        Ok(())
    }

    /// Publish a non-blocking `WikiChanged` notification.
    async fn notify_wiki_changed(&self, path: &str) {
        if let Some(ref tx) = self.wiki_changed_tx {
            if let Err(e) = tx.try_send(path.to_string()) {
                tracing::debug!("PageStore: failed to send WikiChanged for {}: {}", path, e);
            }
        }
    }

    async fn file_mtime(path: &PathBuf) -> Result<i64> {
        let meta = fs::metadata(path).await?;
        let modified = meta.modified()?;
        let secs = modified.duration_since(UNIX_EPOCH)?.as_secs() as i64;
        Ok(secs)
    }

    fn walk_disk_sync(
        root: &PathBuf,
        dir: &PathBuf,
        out: &mut std::collections::HashSet<String>,
    ) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                Self::walk_disk_sync(root, &path, out)?;
            } else if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("md") {
                let rel = path.strip_prefix(root)?;
                let rel_str = {
                    let s = rel.to_string_lossy();
                    s.strip_suffix(".md").unwrap_or(&s).to_string()
                };
                out.insert(rel_str);
            }
        }
        Ok(())
    }
}
