//! In-memory cron job registry
//!
//! Pure sync CRUD operations. No IO, no async, no database.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use super::types::CronJob;

pub(super) struct CronRegistry {
    jobs: HashMap<String, CronJob>,
}

impl CronRegistry {
    pub fn new() -> Self {
        Self {
            jobs: HashMap::new(),
        }
    }

    pub fn insert(&mut self, job: CronJob) -> Option<CronJob> {
        self.jobs.insert(job.id.clone(), job)
    }

    pub fn remove(&mut self, id: &str) -> Option<CronJob> {
        self.jobs.remove(id)
    }

    pub fn get(&self, id: &str) -> Option<&CronJob> {
        self.jobs.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut CronJob> {
        self.jobs.get_mut(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.jobs.contains_key(id)
    }

    pub fn list(&self) -> Vec<CronJob> {
        self.jobs.values().cloned().collect()
    }

    pub fn ids(&self) -> Vec<String> {
        self.jobs.keys().cloned().collect()
    }

    pub fn get_due(&self, now: DateTime<Utc>) -> Vec<CronJob> {
        self.jobs
            .values()
            .filter(|job| job.enabled && job.next_run.is_some_and(|nr| nr <= now))
            .cloned()
            .collect()
    }

    pub fn gc_orphaned(&mut self, current_ids: &HashSet<String>) -> Vec<String> {
        let orphaned: Vec<String> = self
            .jobs
            .keys()
            .filter(|id| !current_ids.contains(*id))
            .cloned()
            .collect();

        for id in &orphaned {
            self.jobs.remove(id);
        }
        orphaned
    }
}
