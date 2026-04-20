use anyhow::Result;
use gasket_storage::wiki::WikiPageStore;
use std::path::PathBuf;
use tokio::fs;

use super::page::{PageFilter, PageSummary, PageType, WikiPage};

/// PageStore: CRUD operations on wiki pages.
/// SQLite is single truth. Disk files are optional cache.
pub struct PageStore {
    db: WikiPageStore,
    wiki_root: PathBuf,
}

impl PageStore {
    pub fn new(pool: sqlx::SqlitePool, wiki_root: PathBuf) -> Self {
        Self {
            db: WikiPageStore::new(pool),
            wiki_root,
        }
    }

    /// Ensure wiki directory structure exists
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

    /// Read a page from SQLite (single truth)
    pub async fn read(&self, path: &str) -> Result<WikiPage> {
        let row = self
            .db
            .get(path)
            .await?
            .ok_or_else(|| anyhow::anyhow!("page not found: {}", path))?;
        Ok(WikiPage {
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
        })
    }

    /// Write a page to SQLite (single truth) + optional disk sync
    pub async fn write(&self, page: &WikiPage) -> Result<()> {
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
            })
            .await?;

        // Lazy disk sync (best effort)
        let _ = self.sync_to_disk(page).await;
        Ok(())
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        self.db.delete(path).await?;
        // Best-effort disk cleanup
        let disk_path = self.wiki_root.join(format!("{}.md", path));
        let _ = fs::remove_file(&disk_path).await;
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
            })
            .collect())
    }

    /// Sync page to disk as markdown (optional, for human readability)
    pub async fn sync_to_disk(&self, page: &WikiPage) -> Result<()> {
        let path = self.wiki_root.join(format!("{}.md", page.path));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&path, page.to_markdown()).await?;
        Ok(())
    }

    /// Rebuild disk cache from SQLite (for migration or recovery)
    pub async fn rebuild_disk_cache(&self) -> Result<usize> {
        let rows = self.db.list_all().await?;
        for row in &rows {
            let page = self.read(&row.path).await?;
            self.sync_to_disk(&page).await?;
        }
        Ok(rows.len())
    }
}
