//! Scheduling calculations — pure functions, no IO.

use chrono::Utc;

use super::types::CronJob;

/// Check if a job's next_run is in the past.
pub fn has_missed_ticks(job: &CronJob) -> bool {
    job.next_run.is_some_and(|nr| nr <= Utc::now())
}
