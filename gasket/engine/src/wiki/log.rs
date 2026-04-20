use anyhow::Result;
use gasket_storage::wiki::WikiLogStore;
use serde::{Deserialize, Serialize};

/// WikiLog: structured operation log.
/// Data in SQLite. No log.md file maintenance.
pub struct WikiLog {
    db: WikiLogStore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: i64,
    pub action: String,
    pub target: Option<String>,
    pub detail: Option<String>,
    pub created: String,
}

impl WikiLog {
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self {
            db: WikiLogStore::new(pool),
        }
    }

    pub async fn append(&self, action: &str, target: &str, detail: &str) -> Result<()> {
        self.db.append(action, target, detail).await
    }

    pub async fn list_recent(&self, limit: i64) -> Result<Vec<LogEntry>> {
        let rows = self.db.list_recent(limit).await?;
        Ok(rows
            .into_iter()
            .map(|r| LogEntry {
                id: r.id,
                action: r.action,
                target: r.target,
                detail: r.detail,
                created: r.created,
            })
            .collect())
    }
}
