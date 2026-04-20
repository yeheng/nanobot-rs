use anyhow::Result;
use sqlx::SqlitePool;

pub struct WikiSourceStore {
    pool: SqlitePool,
}

impl WikiSourceStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn register(&self, id: &str, path: &str, format: &str, title: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            r#"INSERT INTO raw_sources (id, path, format, title, created)
               VALUES ($1, $2, $3, $4, $5)
               ON CONFLICT(id) DO UPDATE SET path = excluded.path, format = excluded.format, title = excluded.title"#,
        )
        .bind(id).bind(path).bind(format).bind(title).bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_ingested(&self, id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query("UPDATE raw_sources SET ingested = 1, ingested_at = $1 WHERE id = $2")
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_uningested(&self) -> Result<Vec<SourceRow>> {
        let rows = sqlx::query_as::<_, SourceRow>(
            "SELECT id, path, format, ingested, ingested_at, title, metadata, created FROM raw_sources WHERE ingested = 0"
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

#[derive(Debug, sqlx::FromRow)]
pub struct SourceRow {
    pub id: String,
    pub path: String,
    pub format: String,
    pub ingested: i32,
    pub ingested_at: Option<String>,
    pub title: Option<String>,
    pub metadata: Option<String>,
    pub created: String,
}
