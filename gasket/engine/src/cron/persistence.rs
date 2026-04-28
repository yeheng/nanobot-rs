//! SQLite state persistence for cron jobs.

use chrono::{DateTime, Utc};
use gasket_storage::CronStore;
use tracing::{debug, warn};

pub(super) struct CronPersistence {
    db: CronStore,
}

impl CronPersistence {
    pub fn new(db: CronStore) -> Self {
        Self { db }
    }

    pub async fn restore_state(
        &self,
        job_id: &str,
    ) -> anyhow::Result<Option<(Option<DateTime<Utc>>, Option<DateTime<Utc>>)>> {
        match self.db.get_state(job_id).await {
            Ok(Some((last_run_str, next_run_str))) => {
                let last_run = last_run_str
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc));
                let next_run = next_run_str
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc));
                debug!("Restored cron state for {} from database", job_id);
                Ok(Some((last_run, next_run)))
            }
            Ok(None) => Ok(None),
            Err(e) => {
                warn!("Failed to load cron state for {}: {}", job_id, e);
                Err(anyhow::anyhow!(
                    "Failed to load cron state for {}: {}",
                    job_id,
                    e
                ))
            }
        }
    }

    pub async fn save_state(
        &self,
        job_id: &str,
        last_run: Option<&DateTime<Utc>>,
        next_run: Option<&DateTime<Utc>>,
    ) -> anyhow::Result<()> {
        self.db
            .upsert_state(
                job_id,
                last_run.map(|t| t.to_rfc3339()).as_deref(),
                next_run.map(|t| t.to_rfc3339()).as_deref(),
            )
            .await?;
        Ok(())
    }

    pub async fn delete_state(&self, job_id: &str) -> anyhow::Result<()> {
        self.db.delete_state(job_id).await?;
        Ok(())
    }
}
