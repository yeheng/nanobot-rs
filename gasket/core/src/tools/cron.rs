//! Cron tool for scheduling tasks

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::instrument;
use uuid::Uuid;

use super::{Tool, ToolContext, ToolError, ToolResult};
use crate::cron::CronService;

/// Cron tool for managing scheduled tasks
pub struct CronTool {
    service: std::sync::Arc<CronService>,
    channel: std::sync::RwLock<Option<String>>,
    chat_id: std::sync::RwLock<Option<String>>,
}

impl CronTool {
    /// Create a new cron tool
    pub fn new(service: std::sync::Arc<CronService>) -> Self {
        Self {
            service,
            channel: std::sync::RwLock::new(None),
            chat_id: std::sync::RwLock::new(None),
        }
    }

    /// Set the context for message routing
    pub fn set_context(&self, channel: &str, chat_id: &str) {
        if let Ok(mut c) = self.channel.write() {
            *c = Some(channel.to_string());
        }
        if let Ok(mut c) = self.chat_id.write() {
            *c = Some(chat_id.to_string());
        }
    }
}

#[async_trait]
impl Tool for CronTool {
    fn name(&self) -> &str {
        "cron"
    }

    fn description(&self) -> &str {
        "Schedule tasks to run at specific times"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "list", "remove"],
                    "description": "Action to perform"
                },
                "name": {
                    "type": "string",
                    "description": "Job name (for add)"
                },
                "cron": {
                    "type": "string",
                    "description": "Cron expression (for add, e.g., '0 9 * * *' for 9 AM daily)"
                },
                "message": {
                    "type": "string",
                    "description": "Message to send at scheduled time (for add)"
                },
                "job_id": {
                    "type": "string",
                    "description": "Job ID to remove (for remove)"
                }
            },
            "required": ["action"]
        })
    }

    #[instrument(name = "tool.cron", skip_all)]
    async fn execute(&self, args: Value, _ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        struct Args {
            action: String,
            name: Option<String>,
            cron: Option<String>,
            message: Option<String>,
            job_id: Option<String>,
        }

        let args: Args =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        match args.action.as_str() {
            "add" => {
                let name = args.name.ok_or_else(|| {
                    ToolError::InvalidArguments("name is required for add".to_string())
                })?;
                let cron = args.cron.ok_or_else(|| {
                    ToolError::InvalidArguments("cron is required for add".to_string())
                })?;
                let message = args.message.ok_or_else(|| {
                    ToolError::InvalidArguments("message is required for add".to_string())
                })?;

                // Validate cron expression
                let _: cron::Schedule = cron.parse().map_err(|e| {
                    ToolError::InvalidArguments(format!("Invalid cron expression: {}", e))
                })?;

                let id = Uuid::new_v4().to_string();
                let channel = self.channel.read().ok().and_then(|c| c.clone());
                let chat_id = self.chat_id.read().ok().and_then(|c| c.clone());

                let mut job = crate::cron::CronJob::new(&id, &name, &cron, &message);
                job.channel = channel;
                job.chat_id = chat_id;

                self.service.add_job(job).await.map_err(|e| {
                    ToolError::ExecutionError(format!("Failed to add cron job '{}': {}", name, e))
                })?;

                Ok(format!("Scheduled job '{}' with ID: {}", name, id))
            }
            "list" => {
                let jobs = self.service.list_jobs().await.map_err(|e| {
                    ToolError::ExecutionError(format!("Failed to list cron jobs: {}", e))
                })?;
                if jobs.is_empty() {
                    return Ok("No scheduled jobs.".to_string());
                }

                let mut result = "Scheduled jobs:\n".to_string();
                for job in jobs {
                    let next = job.next_run.map_or("N/A".to_string(), |t| {
                        t.format("%Y-%m-%d %H:%M UTC").to_string()
                    });
                    result.push_str(&format!(
                        "- {} ({}): {}\n  Cron: {}\n  Next: {}\n\n",
                        job.name, job.id, job.message, job.cron, next
                    ));
                }
                Ok(result)
            }
            "remove" => {
                let job_id = args.job_id.ok_or_else(|| {
                    ToolError::InvalidArguments("job_id is required for remove".to_string())
                })?;

                let removed = self.service.remove_job(&job_id).await.map_err(|e| {
                    ToolError::ExecutionError(format!(
                        "Failed to remove cron job '{}': {}",
                        job_id, e
                    ))
                })?;

                if removed {
                    Ok(format!("Removed job: {}", job_id))
                } else {
                    Ok(format!("Job not found: {}", job_id))
                }
            }
            _ => Err(ToolError::InvalidArguments(format!(
                "Unknown action: {}",
                args.action
            ))),
        }
    }
}
