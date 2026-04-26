use anyhow::Result;
use gasket_broker::{BrokerPayload, Envelope, MemoryBroker, Topic};
use gasket_storage::fs::atomic_write;
use gasket_storage::wiki::{Frequency, WikiPageStore};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use tokio::fs;

use super::page::{PageFilter, PageSummary, PageType, WikiPage};

/// PageStore: CRUD operations on wiki pages.
/// Markdown files on disk are the SSOT. SQLite is a derived index (cache + query projection).
pub struct PageStore {
    db: WikiPageStore,
    wiki_root: PathBuf,
    /// Optional broker for publishing wiki change events.
    broker: Option<Arc<MemoryBroker>>,
}

impl PageStore {
    pub fn new(pool: sqlx::SqlitePool, wiki_root: PathBuf) -> Self {
        Self {
            db: WikiPageStore::new(pool),
            wiki_root,
            broker: None,
        }
    }

    /// Attach a broker so that `write`/`delete` publish async indexing events.
    pub fn with_broker(mut self, broker: Arc<MemoryBroker>) -> Self {
        self.broker = Some(broker);
        self
    }

    /// Get the wiki root directory.
    pub fn wiki_root(&self) -> &PathBuf {
        &self.wiki_root
    }

    /// Get a reference to the underlying `WikiPageStore`.
    pub fn db(&self) -> &WikiPageStore {
        &self.db
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

    /// Read a page — pure query, no sync side effects.
    pub async fn read(&self, path: &str) -> Result<WikiPage> {
        if let Some(row) = self.db.get(path).await? {
            return Ok(self.row_to_page(row));
        }

        let disk_path = self.wiki_root.join(format!("{}.md", path));
        let markdown = fs::read_to_string(&disk_path).await?;
        let mut page = WikiPage::from_markdown(path.to_string(), &markdown)?;
        page.file_mtime = Self::file_mtime(&disk_path).await.unwrap_or(0);
        Ok(page)
    }

    /// Batch-read full pages from SQLite.
    pub async fn read_many(&self, paths: &[String]) -> Result<Vec<WikiPage>> {
        let rows = self.db.get_many(paths).await?;
        Ok(rows.into_iter().map(|row| self.row_to_page(row)).collect())
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
            .map(|r| PageSummary {
                path: r.path.clone(),
                title: r.title.clone(),
                page_type: r.page_type.parse().unwrap_or(PageType::Topic),
                category: r.category.clone(),
                tags: r
                    .tags
                    .as_ref()
                    .and_then(|t| serde_json::from_str(t).ok())
                    .unwrap_or_default(),
                updated: chrono::DateTime::parse_from_rfc3339(&r.updated)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_default(),
                confidence: r.confidence,
                frequency: Frequency::from_str_lossy(&r.frequency),
                access_count: r.access_count as u64,
                last_accessed: r
                    .last_accessed
                    .as_deref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&chrono::Utc)),
                content_length: r.content.len() as u64,
            })
            .collect())
    }

    /// Batch-load lightweight page summaries for a set of paths.
    pub async fn read_summaries(&self, paths: &[String]) -> Result<Vec<PageSummary>> {
        let rows = self.db.get_summaries_by_paths(paths).await?;
        Ok(rows
            .into_iter()
            .map(|r| PageSummary {
                path: r.path,
                title: r.title,
                page_type: r.page_type.parse().unwrap_or(PageType::Topic),
                category: r.category,
                tags: r
                    .tags
                    .as_ref()
                    .and_then(|t| serde_json::from_str(t).ok())
                    .unwrap_or_default(),
                updated: chrono::DateTime::parse_from_rfc3339(&r.updated)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_default(),
                confidence: r.confidence,
                frequency: Frequency::from_str_lossy(&r.frequency),
                access_count: r.access_count as u64,
                last_accessed: r
                    .last_accessed
                    .as_deref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&chrono::Utc)),
                content_length: r.content_length as u64,
            })
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

    fn row_to_page(&self, row: gasket_storage::wiki::PageRow) -> WikiPage {
        WikiPage {
            path: row.path,
            title: row.title,
            page_type: row.page_type.parse().unwrap_or(PageType::Topic),
            category: row.category,
            tags: row
                .tags
                .as_ref()
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or_default(),
            content: row.content,
            created: chrono::DateTime::parse_from_rfc3339(&row.created)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_default(),
            updated: chrono::DateTime::parse_from_rfc3339(&row.updated)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_default(),
            source_count: row.source_count as u32,
            confidence: row.confidence,
            frequency: Frequency::from_str_lossy(&row.frequency),
            access_count: row.access_count as u64,
            last_accessed: row
                .last_accessed
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc)),
            file_mtime: row.file_mtime,
        }
    }

    async fn upsert_db(&self, page: &WikiPage, file_mtime: i64) -> Result<()> {
        let tags_str = serde_json::to_string(&page.tags)?;
        let checksum = Some(format!("{}", page.content.len()));
        self.db
            .upsert(&gasket_storage::wiki::WikiPageInput {
                path: &page.path,
                title: &page.title,
                page_type: page.page_type.as_str(),
                category: page.category.as_deref(),
                tags: &tags_str,
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

    /// Publish a non-blocking `WikiChanged` event if a broker is attached.
    async fn notify_wiki_changed(&self, path: &str) {
        if let Some(ref broker) = self.broker {
            let envelope = Envelope::new(
                Topic::WikiChanged,
                BrokerPayload::WikiChanged {
                    path: path.to_string(),
                },
            );
            if let Err(e) = broker.try_publish(envelope) {
                tracing::debug!(
                    "PageStore: failed to publish WikiChanged for {}: {}",
                    path,
                    e
                );
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
