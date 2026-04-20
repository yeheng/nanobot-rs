use sqlx::SqlitePool;

/// Create all wiki-related tables.
/// Key design: wiki_pages.path is PK, content lives in SQLite (single truth).
/// No wiki_page_locks table — SQLite WAL handles concurrency.
pub async fn create_wiki_tables(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS wiki_pages (
            path        TEXT PRIMARY KEY,
            title       TEXT NOT NULL,
            type        TEXT NOT NULL,
            category    TEXT,
            tags        TEXT,
            content     TEXT NOT NULL DEFAULT '',
            created     TEXT NOT NULL,
            updated     TEXT NOT NULL,
            source_count INTEGER DEFAULT 0,
            confidence  REAL DEFAULT 1.0,
            checksum    TEXT
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS raw_sources (
            id          TEXT PRIMARY KEY,
            path        TEXT NOT NULL,
            format      TEXT NOT NULL,
            ingested    INTEGER DEFAULT 0,
            ingested_at TEXT,
            title       TEXT,
            metadata    TEXT,
            created     TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS wiki_relations (
            from_page   TEXT NOT NULL,
            to_page     TEXT NOT NULL,
            relation    TEXT NOT NULL,
            confidence  REAL DEFAULT 1.0,
            created     TEXT NOT NULL,
            PRIMARY KEY (from_page, to_page, relation)
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS wiki_log (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            action      TEXT NOT NULL,
            target      TEXT,
            detail      TEXT,
            created     TEXT NOT NULL DEFAULT (datetime('now'))
        )
        "#,
    )
    .execute(pool)
    .await?;

    // Indexes
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_wiki_pages_type ON wiki_pages(type)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_wiki_pages_category ON wiki_pages(category)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_wiki_pages_updated ON wiki_pages(updated)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_raw_sources_ingested ON raw_sources(ingested)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_wiki_log_action ON wiki_log(action)")
        .execute(pool)
        .await?;

    Ok(())
}
