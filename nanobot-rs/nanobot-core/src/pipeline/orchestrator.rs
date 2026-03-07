//! Orchestrator actor for the multi-agent pipeline.
//!
//! Receives `PipelineEvent` messages on a dedicated mpsc channel and
//! dispatches agents according to the state machine and permission matrix.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::agent::subagent::SubagentManager;

use super::config::PipelineConfig;
use super::permission::PermissionMatrix;
use super::state_machine::TaskState;
use super::store::PipelineStore;

/// Events processed by the orchestrator's event loop.
#[derive(Debug, Clone)]
pub enum PipelineEvent {
    /// A new task was created (Pending).
    TaskCreated { task_id: String },

    /// A task transitioned to a new state.
    TaskTransitioned {
        task_id: String,
        new_state: TaskState,
        agent_role: String,
    },

    /// An agent reported progress.
    ProgressReported {
        task_id: String,
        agent_role: String,
    },

    /// Stall detector found a stalled task.
    StallDetected { task_id: String },
}

/// The governance roles that use synchronous (wait) dispatch.
const GOVERNANCE_ROLES: &[&str] = &["taizi", "zhongshu", "menxia", "shangshu"];

/// The central orchestrator that drives the pipeline lifecycle.
pub struct OrchestratorActor {
    store: PipelineStore,
    #[allow(dead_code)]
    permission_matrix: PermissionMatrix,
    subagent_manager: Arc<SubagentManager>,
    config: PipelineConfig,
    event_rx: mpsc::Receiver<PipelineEvent>,
    /// Role name → SOUL.md content.
    soul_templates: HashMap<String, String>,
}

impl OrchestratorActor {
    pub fn new(
        store: PipelineStore,
        permission_matrix: PermissionMatrix,
        subagent_manager: Arc<SubagentManager>,
        config: PipelineConfig,
        event_rx: mpsc::Receiver<PipelineEvent>,
        soul_templates: HashMap<String, String>,
    ) -> Self {
        Self {
            store,
            permission_matrix,
            subagent_manager,
            config,
            event_rx,
            soul_templates,
        }
    }

    /// Run the event loop. This should be spawned on a dedicated tokio task.
    pub async fn run(mut self) {
        info!("Pipeline orchestrator started");
        while let Some(event) = self.event_rx.recv().await {
            debug!("Orchestrator received event: {:?}", event);
            if let Err(e) = self.handle_event(event).await {
                error!("Orchestrator error: {}", e);
            }
        }
        info!("Pipeline orchestrator stopped (channel closed)");
    }

    async fn handle_event(&self, event: PipelineEvent) -> anyhow::Result<()> {
        match event {
            PipelineEvent::TaskCreated { task_id } => {
                self.handle_task_created(&task_id).await?;
            }
            PipelineEvent::TaskTransitioned {
                task_id,
                new_state,
                agent_role: _,
            } => {
                self.handle_task_transitioned(&task_id, new_state).await?;
            }
            PipelineEvent::ProgressReported {
                task_id,
                agent_role,
            } => {
                debug!(
                    "Progress from {} on task {}",
                    agent_role, task_id
                );
            }
            PipelineEvent::StallDetected { task_id } => {
                self.handle_stall(&task_id).await?;
            }
        }
        Ok(())
    }

    /// New task created → advance Pending → Triage and dispatch the taizi agent.
    async fn handle_task_created(&self, task_id: &str) -> anyhow::Result<()> {
        let ok = self
            .store
            .update_task_state(task_id, TaskState::Pending, TaskState::Triage, Some("taizi"))
            .await?;

        if !ok {
            warn!("Task {} already moved past Pending", task_id);
            return Ok(());
        }

        self.store
            .append_flow_log(task_id, "pending", "triage", "system", Some("auto dispatch"))
            .await?;

        self.dispatch_agent(task_id, "taizi").await
    }

    /// A task transitioned → look up the new responsible role and dispatch.
    async fn handle_task_transitioned(
        &self,
        task_id: &str,
        new_state: TaskState,
    ) -> anyhow::Result<()> {
        if new_state == TaskState::Done {
            info!("Task {} completed", task_id);
            return Ok(());
        }

        let role = new_state.responsible_role();

        // For Reviewing state, increment review count and check limits
        if new_state == TaskState::Reviewing {
            let count = self.store.increment_review_count(task_id).await?;
            if count > self.config.max_reviews {
                warn!(
                    "Task {} exceeded max reviews ({}), escalating",
                    task_id, self.config.max_reviews
                );
                // Force to Blocked for manual intervention
                let _ = self
                    .store
                    .update_task_state(
                        task_id,
                        TaskState::Reviewing,
                        TaskState::Blocked,
                        Some("system"),
                    )
                    .await;
                self.store
                    .append_flow_log(
                        task_id,
                        "reviewing",
                        "blocked",
                        "system",
                        Some("review limit exceeded"),
                    )
                    .await?;
                return Ok(());
            }
        }

        self.dispatch_agent(task_id, role).await
    }

    /// Handle a stalled task: retry → escalate → block.
    async fn handle_stall(&self, task_id: &str) -> anyhow::Result<()> {
        let task = match self.store.get_task(task_id).await? {
            Some(t) => t,
            None => return Ok(()),
        };

        info!(
            "Handling stall for task {} (retry_count={})",
            task_id, task.retry_count
        );

        if task.retry_count == 0 {
            // Level 1: retry the same agent
            warn!("Stall L1: retrying task {}", task_id);
            self.store.update_heartbeat(task_id).await?;
            let role = task
                .assigned_role
                .as_deref()
                .unwrap_or(task.state.responsible_role());
            self.dispatch_agent(task_id, role).await?;
            // Increment retry so next stall escalates
            sqlx::query("UPDATE pipeline_tasks SET retry_count = retry_count + 1 WHERE id = ?")
                .bind(task_id)
                .execute(&self.store.pool)
                .await
                .ok();
        } else {
            // Level 2+: block the task for manual intervention
            warn!("Stall L2: blocking task {}", task_id);
            let _ = self
                .store
                .update_task_state(task_id, task.state, TaskState::Blocked, Some("system"))
                .await;
            self.store
                .append_flow_log(
                    task_id,
                    &task.state.to_string(),
                    "blocked",
                    "system",
                    Some("stall detected, escalated"),
                )
                .await?;
        }

        Ok(())
    }

    /// Dispatch an agent for the given role on the given task.
    async fn dispatch_agent(&self, task_id: &str, role: &str) -> anyhow::Result<()> {
        let task = match self.store.get_task(task_id).await? {
            Some(t) => t,
            None => {
                warn!("Cannot dispatch: task {} not found", task_id);
                return Ok(());
            }
        };

        let prompt = format!(
            "You are the {} agent in the pipeline.\n\n\
             ## Task\n\
             - **ID**: {}\n\
             - **Title**: {}\n\
             - **State**: {}\n\
             - **Priority**: {}\n\n\
             ## Description\n{}\n\n\
             Use the pipeline_task tool to transition the task to the next state \
             when you are done. Use report_progress to update status.",
            role, task.id, task.title, task.state, task.priority, task.description,
        );

        let system_prompt = self.soul_templates.get(role);

        if GOVERNANCE_ROLES.contains(&role) {
            // Synchronous dispatch for governance agents
            info!("Dispatching governance agent '{}' for task {}", role, task_id);
            let response = self
                .subagent_manager
                .submit_and_wait(&prompt, system_prompt.map(|s| s.as_str()))
                .await;

            match response {
                Ok(resp) => {
                    debug!(
                        "Governance agent '{}' responded: {}",
                        role,
                        &resp.content[..resp.content.len().min(200)]
                    );
                }
                Err(e) => {
                    error!("Governance agent '{}' failed: {}", role, e);
                }
            }
        } else {
            // Async dispatch for execution agents (六部)
            info!("Dispatching execution agent '{}' for task {}", role, task_id);
            self.subagent_manager
                .submit(&prompt, "cli", "pipeline_exec")?;
        }

        Ok(())
    }
}
