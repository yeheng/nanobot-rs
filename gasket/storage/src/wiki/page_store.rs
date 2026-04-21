use anyhow::Result;
use sqlx::{Row, SqlitePool};

use super::types::Frequency;

/// A candidate for frequency decay.
#[derive(Debug, Clone)]
pub struct DecayCandidate {
    pub path: String,
    pub frequency: Frequency,
    pub last_accessed: String,
}

/// Input for upserting a wiki page into SQLite.
#[derive(Debug)]
pub struct WikiPageInput<'a> {
    pub path: &'a str,
    pub title: &'a str,
    pub page_type: &'a str,
    pub category: Option<&'a str>,
    pub tags: &'a str,
    pub content: &'a str,
    pub source_count: u32,
    pub confidence: f64,
    pub checksum: Option<&'a str>,
    /// Machine runtime state: access frequency tier.
    pub frequency: Frequency,
    /// Machine runtime state: total access count.
    pub access_count: u64,
    /// Machine runtime state: last access timestamp (RFC3339).
    pub last_accessed: Option<String>,
    /// Machine runtime state: disk file mtime (Unix epoch seconds).
    pub file_mtime: i64,
}

/// SQLite-backed wiki page store. Single source of truth.
/// Content lives here. Disk files are optional cache.
pub struct WikiPageStore {
    pool: SqlitePool,
}

impl WikiPageStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Atomic UPSERT. SQLite WAL handles concurrency.
    pub async fn upsert(&self, page: &WikiPageInput<'_>) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO wiki_pages (path, title, type, category, tags, content, created, updated, source_count, confidence, checksum, frequency, access_count, last_accessed, file_mtime)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
            ON CONFLICT(path) DO UPDATE SET
                title = excluded.title,
                type = excluded.type,
                category = excluded.category,
                tags = excluded.tags,
                content = excluded.content,
                updated = excluded.updated,
                source_count = excluded.source_count,
                confidence = excluded.confidence,
                checksum = excluded.checksum
            "#,
        )
        .bind(page.path)
        .bind(page.title)
        .bind(page.page_type)
        .bind(page.category)
        .bind(page.tags)
        .bind(page.content)
        .bind(&now)
        .bind(&now)
        .bind(page.source_count as i64)
        .bind(page.confidence)
        .bind(page.checksum)
        .bind(page.frequency.to_string())
        .bind(page.access_count as i64)
        .bind(page.last_accessed.as_deref())
        .bind(page.file_mtime)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, path: &str) -> Result<Option<PageRow>> {
        let row = sqlx::query_as::<_, PageRow>(
            "SELECT path, title, type, category, tags, content, created, updated, source_count, confidence, checksum, frequency, access_count, last_accessed, file_mtime FROM wiki_pages WHERE path = $1"
        )
        .bind(path)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        sqlx::query("DELETE FROM wiki_pages WHERE path = $1")
            .bind(path)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn exists(&self, path: &str) -> Result<bool> {
        let row: Option<(String,)> = sqlx::query_as("SELECT path FROM wiki_pages WHERE path = $1")
            .bind(path)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    pub async fn list_by_type(&self, page_type: &str) -> Result<Vec<PageRow>> {
        let rows = sqlx::query_as::<_, PageRow>(
            "SELECT path, title, type, category, tags, content, created, updated, source_count, confidence, checksum, frequency, access_count, last_accessed, file_mtime FROM wiki_pages WHERE type = $1 ORDER BY updated DESC"
        )
        .bind(page_type)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn list_all(&self) -> Result<Vec<PageRow>> {
        let rows = sqlx::query_as::<_, PageRow>(
            "SELECT path, title, type, category, tags, content, created, updated, source_count, confidence, checksum, frequency, access_count, last_accessed, file_mtime FROM wiki_pages ORDER BY updated DESC"
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Get decay candidates: pages whose last_accessed is older than `days` and
    /// are not already archived.
    pub async fn get_decay_candidates(&self, days: i64) -> Result<Vec<DecayCandidate>> {
        let sql = format!(
            "SELECT path, frequency, last_accessed
             FROM wiki_pages
             WHERE frequency != 'archived'
               AND last_accessed IS NOT NULL
               AND last_accessed != ''
               AND datetime(last_accessed) < datetime('now', '-{} days')",
            days
        );
        let rows = sqlx::query(&sql).fetch_all(&self.pool).await?;
        let candidates: Vec<_> = rows
            .into_iter()
            .map(|row| {
                let freq_str: String = row.get("frequency");
                DecayCandidate {
                    path: row.get("path"),
                    frequency: Frequency::from_str_lossy(&freq_str),
                    last_accessed: row.get("last_accessed"),
                }
            })
            .collect();
        Ok(candidates)
    }

    /// Update only the frequency of a page (used by decay).
    pub async fn update_frequency(&self, path: &str, frequency: Frequency) -> Result<bool> {
        let result = sqlx::query("UPDATE wiki_pages SET frequency = ? WHERE path = ?")
            .bind(frequency.to_string())
            .bind(path)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

#[derive(Debug, sqlx::FromRow)]
pub struct PageRow {
    pub path: String,
    pub title: String,
    #[sqlx(rename = "type")]
    pub page_type: String,
    pub category: Option<String>,
    pub tags: Option<String>,
    pub content: String,
    pub created: String,
    pub updated: String,
    pub source_count: i64,
    pub confidence: f64,
    pub checksum: Option<String>,
    pub frequency: String,
    pub access_count: i64,
    pub last_accessed: Option<String>,
    pub file_mtime: i64,
}
