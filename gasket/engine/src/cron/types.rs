//! Cron module data types
//!
//! Pure data structures with no IO or business logic.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use cron::Schedule;
use serde::{Deserialize, Deserializer};

/// A scheduled job (config from file, state from database)
#[derive(Debug, Clone)]
pub struct CronJob {
    /// Unique job ID (filename without .md)
    pub id: String,
    /// Job name
    pub name: String,
    /// Cron expression
    pub cron: String,
    /// Message to send (for LLM-based jobs)
    pub message: String,
    /// Target channel
    pub channel: Option<String>,
    /// Target chat ID
    pub chat_id: Option<String>,
    /// Tool name to execute directly (bypasses LLM)
    pub tool: Option<String>,
    /// Tool arguments (JSON value)
    pub tool_args: Option<serde_json::Value>,
    /// Last run time (restored from database)
    pub last_run: Option<DateTime<Utc>>,
    /// Next run time (restored from database)
    pub next_run: Option<DateTime<Utc>>,
    /// Enabled
    pub enabled: bool,
    /// File path for hot reload
    pub file_path: PathBuf,
    /// Parsed cron schedule (cached to avoid parsing on every check)
    pub(crate) schedule: Option<Schedule>,
}

/// Frontmatter structure for markdown job files
#[derive(Debug, Deserialize)]
pub(super) struct CronJobFrontmatter {
    pub name: Option<String>,
    pub cron: String,
    pub channel: Option<String>,
    pub to: Option<String>,
    #[serde(default = "default_true", deserialize_with = "deserialize_bool_or_string")]
    pub enabled: bool,
    pub tool: Option<String>,
    pub tool_args: Option<serde_json::Value>,
}

fn default_true() -> bool {
    true
}

/// Deserialize a bool from either a boolean or a string like "true" / "false".
fn deserialize_bool_or_string<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_yaml::Value::deserialize(deserializer)?;
    match value {
        serde_yaml::Value::Bool(b) => Ok(b),
        serde_yaml::Value::String(s) => match s.to_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(true),
            "false" | "0" | "no" | "off" => Ok(false),
            _ => Err(serde::de::Error::custom(format!("invalid boolean string: {}", s))),
        },
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i != 0)
            } else {
                Err(serde::de::Error::custom("invalid boolean number"))
            }
        }
        _ => Err(serde::de::Error::custom("expected boolean or string")),
    }
}

/// Report from refresh_all_jobs operation
#[derive(Debug, Clone, Default)]
pub struct RefreshReport {
    pub loaded: usize,
    pub updated: usize,
    pub removed: usize,
    pub errors: usize,
}

/// Entry returned by refresh_next_run: (job_id, job_name, next_run)
pub type RefreshNextRunEntry = (String, String, Option<DateTime<Utc>>);

impl CronJob {
    /// Create a new cron job
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        cron: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        let cron_str = cron.into();
        let (schedule, next_run) = Self::parse_schedule(&cron_str);

        Self {
            id: id.into(),
            name: name.into(),
            cron: cron_str,
            message: message.into(),
            channel: None,
            chat_id: None,
            tool: None,
            tool_args: None,
            last_run: None,
            next_run,
            enabled: true,
            file_path: PathBuf::new(),
            schedule,
        }
    }

    /// Parse cron expression and calculate next run time.
    pub(crate) fn parse_schedule(cron_expr: &str) -> (Option<Schedule>, Option<DateTime<Utc>>) {
        let schedule: Schedule = match cron_expr.parse() {
            Ok(s) => s,
            Err(_) => return (None, None),
        };
        let now = chrono::Utc::now();
        let next_run = schedule.after(&now).next();
        (Some(schedule), next_run)
    }

    /// Calculate next run time using the cached schedule.
    pub(crate) fn calculate_next_run(&self) -> Option<DateTime<Utc>> {
        let schedule = self.schedule.as_ref()?;
        let now = chrono::Utc::now();
        schedule.after(&now).next()
    }

    /// Update next run time using the cached schedule
    pub fn update_next_run(&mut self) {
        self.next_run = self.calculate_next_run();
    }
}
