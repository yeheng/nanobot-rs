//! State machine engine that processes events and manages state transitions.
//!
//! The engine is the heart of the state machine subsystem. It:
//! - Processes events from the event channel
//! - Validates and executes state transitions
//! - Dispatches agents based on the current state
//! - Handles stall detection and recovery

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use nanobot_core::agent::subagent::SubagentManager;

use crate::events::StateMachineEvent;
use crate::models::StateMachineTask;
use crate::store::StateMachineStore;
use crate::types::StateMachineConfig;

/// The state machine engine that drives the multi-agent collaboration.
pub struct StateMachineEngine {
    store: StateMachineStore,
    subagent_manager: Arc<SubagentManager>,
    config: StateMachineConfig,
    event_tx: mpsc::Sender<StateMachineEvent>,
    event_rx: mpsc::Receiver<StateMachineEvent>,
    /// Role name → SOUL.md content mapping.
    soul_templates: HashMap<String, String>,
}

impl StateMachineEngine {
    /// Create a new state machine engine.
    pub fn new(
        store: StateMachineStore,
        subagent_manager: Arc<SubagentManager>,
        config: StateMachineConfig,
        event_tx: mpsc::Sender<StateMachineEvent>,
        event_rx: mpsc::Receiver<StateMachineEvent>,
        soul_templates: HashMap<String, String>,
    ) -> Self {
        Self {
            store,
            subagent_manager,
            config,
            event_tx,
            event_rx,
            soul_templates,
        }
    }

    /// Run the event loop. This should be spawned on a dedicated tokio task.
    pub async fn run(mut self) {
        info!(
            "State machine engine started (initial_state={}, terminal_states={:?})",
            self.config.initial_state, self.config.terminal_states
        );

        while let Some(event) = self.event_rx.recv().await {
            debug!("Engine received event: {:?}", event);
            if let Err(e) = self.handle_event(event).await {
                error!("Engine error handling event: {}", e);
            }
        }

        info!("State machine engine stopped (channel closed)");
    }

    /// Handle an incoming event.
    async fn handle_event(&self, event: StateMachineEvent) -> anyhow::Result<()> {
        match event {
            StateMachineEvent::TaskCreated {
                task_id,
                session_id,
            } => {
                self.handle_task_created(&task_id, session_id.as_deref())
                    .await?;
            }
            StateMachineEvent::TaskTransitioned {
                task_id,
                from_state,
                to_state,
                agent_role,
            } => {
                self.handle_task_transitioned(&task_id, &from_state, &to_state, &agent_role)
                    .await?;
            }
            StateMachineEvent::ProgressReported {
                task_id,
                agent_role,
                content,
            } => {
                self.handle_progress_reported(&task_id, &agent_role, &content)
                    .await?;
            }
            StateMachineEvent::StallDetected { task_id } => {
                self.handle_stall(&task_id).await?;
            }
            StateMachineEvent::TransitionRequest {
                task_id,
                to_state,
                agent_role,
                reason,
            } => {
                self.handle_transition_request(&task_id, &to_state, &agent_role, reason.as_deref())
                    .await?;
            }
            StateMachineEvent::CreateTaskRequest {
                title,
                description,
                priority,
                origin_channel,
                origin_chat_id,
                session_id,
            } => {
                self.handle_create_task_request(
                    &title,
                    &description,
                    priority.as_deref(),
                    origin_channel.as_deref(),
                    origin_chat_id.as_deref(),
                    session_id.as_deref(),
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Handle a task creation event.
    async fn handle_task_created(
        &self,
        task_id: &str,
        _session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let initial_state = &self.config.initial_state;
        let role = self
            .config
            .responsible_role(initial_state)
            .unwrap_or("system");

        // Transition from implicit "pending" to initial state
        let ok = self
            .store
            .update_task_state(task_id, "pending", initial_state, Some(role))
            .await?;

        if !ok {
            warn!("Task {} already moved past pending", task_id);
            return Ok(());
        }

        self.store
            .append_flow_log(
                task_id,
                "pending",
                initial_state,
                "system",
                Some("auto dispatch on creation"),
            )
            .await?;

        info!(
            "Task {} created and dispatched to {} at {}",
            task_id, role, initial_state
        );

        self.dispatch_agent(task_id, role, initial_state).await
    }

    /// Handle a task transition event.
    async fn handle_task_transitioned(
        &self,
        task_id: &str,
        from_state: &str,
        to_state: &str,
        agent_role: &str,
    ) -> anyhow::Result<()> {
        // Check if this is a terminal state
        if self.config.is_terminal(to_state) {
            info!(
                "Task {} reached terminal state {} via {}",
                task_id, to_state, agent_role
            );
            return Ok(());
        }

        // For gated states, increment review count and check limits
        if let Some(gate) = self.config.gate_config(to_state) {
            let count = self.store.increment_review_count(task_id).await?;
            if count > self.config.max_reviews {
                warn!(
                    "Task {} exceeded max reviews ({}), escalating to {}",
                    task_id, self.config.max_reviews, gate.reject_to
                );
                // Force to the gate's reject_to state for intervention
                let _ = self
                    .store
                    .update_task_state(task_id, to_state, &gate.reject_to, Some("system"))
                    .await;
                self.store
                    .append_flow_log(
                        task_id,
                        to_state,
                        &gate.reject_to,
                        "system",
                        Some("review limit exceeded"),
                    )
                    .await?;
                return Ok(());
            }
        }

        // Dispatch the agent responsible for the new state
        let next_role = self.config.responsible_role(to_state).unwrap_or("system");

        info!(
            "Task {} transitioned {} -> {}, dispatching {}",
            task_id, from_state, to_state, next_role
        );

        self.dispatch_agent(task_id, next_role, to_state).await
    }

    /// Handle a progress report.
    async fn handle_progress_reported(
        &self,
        task_id: &str,
        agent_role: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        debug!(
            "Progress from {} on task {}: {}",
            agent_role, task_id, content
        );

        // Append to progress log (this also updates heartbeat)
        self.store
            .append_progress(task_id, agent_role, content, None)
            .await?;

        Ok(())
    }

    /// Handle a stall detection event.
    async fn handle_stall(&self, task_id: &str) -> anyhow::Result<()> {
        let task = match self.store.get_task(task_id).await? {
            Some(t) => t,
            None => return Ok(()),
        };

        info!(
            "Handling stall for task {} (state={}, retry_count={})",
            task_id, task.state, task.retry_count
        );

        if task.retry_count == 0 {
            // Level 1: retry the same agent
            warn!("Stall L1: retrying task {}", task_id);
            self.store.update_heartbeat(task_id).await?;

            let role = task
                .assigned_role
                .as_deref()
                .or_else(|| self.config.responsible_role(&task.state))
                .unwrap_or("system");

            self.dispatch_agent(task_id, role, &task.state).await?;

            // Increment retry so next stall escalates
            sqlx::query(
                "UPDATE state_machine_tasks SET retry_count = retry_count + 1 WHERE id = ?",
            )
            .bind(task_id)
            .execute(&self.store.pool)
            .await
            .ok();
        } else {
            // Level 2+: block the task for manual intervention
            warn!("Stall L2: blocking task {}", task_id);
            let _ = self
                .store
                .update_task_state(task_id, &task.state, "blocked", Some("system"))
                .await;
            self.store
                .append_flow_log(
                    task_id,
                    &task.state,
                    "blocked",
                    "system",
                    Some("stall detected, escalated"),
                )
                .await?;
        }

        Ok(())
    }

    /// Handle an external transition request.
    async fn handle_transition_request(
        &self,
        task_id: &str,
        to_state: &str,
        agent_role: &str,
        reason: Option<&str>,
    ) -> anyhow::Result<()> {
        let task = match self.store.get_task(task_id).await? {
            Some(t) => t,
            None => {
                return Err(anyhow::anyhow!("Task {} not found", task_id));
            }
        };

        // Validate the transition is allowed from current state
        if !self.config.can_transition(&task.state, to_state) {
            let allowed = self.config.allowed_transitions(&task.state);
            return Err(anyhow::anyhow!(
                "Cannot transition from '{}' to '{}'. Allowed: {:?}",
                task.state,
                to_state,
                allowed
            ));
        }

        // Execute the transition
        let ok = self
            .store
            .update_task_state(task_id, &task.state, to_state, Some(agent_role))
            .await?;

        if !ok {
            return Err(anyhow::anyhow!(
                "Concurrent modification: task {} state changed",
                task_id
            ));
        }

        // Write audit log
        self.store
            .append_flow_log(task_id, &task.state, to_state, agent_role, reason)
            .await?;

        info!(
            "Task {} transitioned {} -> {} by {}",
            task_id, task.state, to_state, agent_role
        );

        // Emit TaskTransitioned event to trigger next dispatch
        // We need to send this to the event channel, but we can't block here
        // The event will be processed asynchronously
        let event = StateMachineEvent::TaskTransitioned {
            task_id: task_id.to_string(),
            from_state: task.state,
            to_state: to_state.to_string(),
            agent_role: agent_role.to_string(),
        };

        // Send event (ignore error if channel closed)
        let _ = self.event_tx.send(event).await;

        Ok(())
    }

    /// Handle a create task request from external source.
    async fn handle_create_task_request(
        &self,
        title: &str,
        description: &str,
        priority: Option<&str>,
        origin_channel: Option<&str>,
        origin_chat_id: Option<&str>,
        session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        use chrono::Utc;
        use uuid::Uuid;

        let id = Uuid::new_v4().to_string();
        let now = Utc::now();

        let task = StateMachineTask {
            id: id.clone(),
            title: title.to_string(),
            description: description.to_string(),
            state: "pending".to_string(),
            priority: priority
                .map(|p| match p {
                    "low" => super::models::TaskPriority::Low,
                    "high" => super::models::TaskPriority::High,
                    "critical" => super::models::TaskPriority::Critical,
                    _ => super::models::TaskPriority::Normal,
                })
                .unwrap_or_default(),
            assigned_role: None,
            review_count: 0,
            retry_count: 0,
            last_heartbeat: now,
            created_at: now,
            updated_at: now,
            result: None,
            origin_channel: origin_channel.map(|s| s.to_string()),
            origin_chat_id: origin_chat_id.map(|s| s.to_string()),
            session_id: session_id.map(|s| s.to_string()),
        };

        self.store.create_task(&task).await?;

        info!("Task {} created: {}", id, title);

        // Emit TaskCreated event to trigger initial dispatch
        Ok(())
    }

    /// Dispatch an agent for the given role on the given task.
    async fn dispatch_agent(&self, task_id: &str, role: &str, state: &str) -> anyhow::Result<()> {
        let task = match self.store.get_task(task_id).await? {
            Some(t) => t,
            None => {
                warn!("Cannot dispatch: task {} not found", task_id);
                return Ok(());
            }
        };

        let prompt = format!(
            "You are the '{}' agent in the state machine.\n\n\
             ## Task\n\
             - **ID**: {}\n\
             - **Title**: {}\n\
             - **State**: {}\n\
             - **Priority**: {}\n\n\
             ## Description\n{}\n\n\
             Use the state_machine_task tool to transition the task to the next state \
             when you are done. Use report_progress to update status.",
            role, task.id, task.title, state, task.priority, task.description,
        );

        let system_prompt = self.soul_templates.get(role);

        if self.config.is_sync_role(role) {
            // Synchronous dispatch for governance agents
            info!(
                "Dispatching synchronous agent '{}' for task {} at {}",
                role, task_id, state
            );
            let response = self
                .subagent_manager
                .submit_and_wait(&prompt, system_prompt.map(|s| s.as_str()))
                .await;

            match response {
                Ok(resp) => {
                    debug!(
                        "Synchronous agent '{}' responded: {}",
                        role,
                        &resp.content[..resp.content.len().min(200)]
                    );
                }
                Err(e) => {
                    error!("Synchronous agent '{}' failed: {}", role, e);
                }
            }
        } else {
            // Async dispatch for execution agents
            info!(
                "Dispatching async agent '{}' for task {} at {}",
                role, task_id, state
            );
            self.subagent_manager
                .submit(&prompt, "cli", "state_machine_exec")?;
        }

        Ok(())
    }
}
