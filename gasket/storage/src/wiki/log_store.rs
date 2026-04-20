use anyhow::Result;
use sqlx::SqlitePool;

pub struct WikiLogStore {
    pool: SqlitePool,
}

impl WikiLogStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn append(&self, action: &str, target: &str, detail: &str) -> Result<()> {
        sqlx::query("INSERT INTO wiki_log (action, target, detail) VALUES ($1, $2, $3)")
            .bind(action)
            .bind(target)
            .bind(detail)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_recent(&self, limit: i64) -> Result<Vec<LogRow>> {
        let rows = sqlx::query_as::<_, LogRow>(
            "SELECT id, action, target, detail, created FROM wiki_log ORDER BY id DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

#[derive(Debug, sqlx::FromRow)]
pub struct LogRow {
    pub id: i64,
    pub action: String,
    pub target: Option<String>,
    pub detail: Option<String>,
    pub created: String,
}
