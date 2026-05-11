use anyhow::Result;
use sqlx::SqlitePool;

#[derive(Clone)]
pub struct WikiRelationStore {
    pool: SqlitePool,
}

impl WikiRelationStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn add(&self, from_page: &str, to_page: &str, relation: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            r#"INSERT INTO wiki_relations (from_page, to_page, relation, created)
               VALUES ($1, $2, $3, $4)
               ON CONFLICT(from_page, to_page, relation) DO UPDATE SET confidence = 1.0"#,
        )
        .bind(from_page)
        .bind(to_page)
        .bind(relation)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Batch-add relations within a single transaction.
    pub async fn add_many(
        &self,
        from_page: &str,
        to_pages: &[String],
        relation: &str,
    ) -> Result<()> {
        if to_pages.is_empty() {
            return Ok(());
        }

        let now = chrono::Utc::now().to_rfc3339();
        let mut tx = self.pool.begin().await?;

        for to_page in to_pages {
            sqlx::query(
                r#"INSERT INTO wiki_relations (from_page, to_page, relation, created)
                   VALUES ($1, $2, $3, $4)
                   ON CONFLICT(from_page, to_page, relation) DO UPDATE SET confidence = 1.0"#,
            )
            .bind(from_page)
            .bind(to_page)
            .bind(relation)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn get_outgoing(&self, path: &str) -> Result<Vec<RelationRow>> {
        let rows = sqlx::query_as::<_, RelationRow>(
            "SELECT from_page, to_page, relation, confidence, created FROM wiki_relations WHERE from_page = $1"
        )
        .bind(path)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get_incoming(&self, path: &str) -> Result<Vec<RelationRow>> {
        let rows = sqlx::query_as::<_, RelationRow>(
            "SELECT from_page, to_page, relation, confidence, created FROM wiki_relations WHERE to_page = $1"
        )
        .bind(path)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn delete_all_for_page(&self, path: &str) -> Result<()> {
        sqlx::query("DELETE FROM wiki_relations WHERE from_page = $1 OR to_page = $1")
            .bind(path)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete all outgoing relations from a page (preserves incoming).
    pub async fn delete_all_outgoing(&self, path: &str) -> Result<()> {
        sqlx::query("DELETE FROM wiki_relations WHERE from_page = $1")
            .bind(path)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[derive(Debug, sqlx::FromRow)]
pub struct RelationRow {
    pub from_page: String,
    pub to_page: String,
    pub relation: String,
    pub confidence: f64,
    pub created: String,
}
