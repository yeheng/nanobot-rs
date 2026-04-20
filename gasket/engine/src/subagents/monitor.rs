//! Monitored subagent execution — real-time progress + intervention via channels.
//!
//! Replaces GenericAgent's file-IO protocol with type-safe Rust channels.
//! No SQLite fallback — if the subagent crashes, state is lost (KISS).

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::info;

use crate::kernel::{StepResult, SteppableExecutor, TokenLedger};
use crate::session::config::AgentConfigExt;
use crate::session::AgentResponse;
use crate::tools::ToolRegistry;
use gasket_providers::{ChatMessage, LlmProvider};

use super::manager::TaskSpec;
use super::tracker::SubagentResult;

// ── Types ────────────────────────────────────────────────────────

/// Progress events emitted by a monitored subagent.
#[derive(Debug, Clone)]
pub enum ProgressUpdate {
    Thinking { turn: usize },
    ToolStart { name: String },
    ToolResult { name: String, output: String },
    TurnComplete { turn: usize, summary: String },
    Done { result: String },
    Error { message: String },
}

/// Intervention commands sent to a monitored subagent.
#[derive(Debug, Clone)]
pub enum Intervention {
    Abort,
    AddKeyInfo(String),
    AppendPrompt(String),
    ExtendTurns(u32),
}

/// Handle to a monitored subagent — includes progress stream and intervention channel.
pub struct MonitoredHandle {
    pub handle: JoinHandle<SubagentResult>,
    pub interventor: mpsc::Sender<Intervention>,
    pub progress: mpsc::Receiver<ProgressUpdate>,
}

// ── MonitoredSpawner ─────────────────────────────────────────────

/// Spawns subagents with real-time monitoring and intervention.
pub struct MonitoredSpawner;

impl MonitoredSpawner {
    pub fn spawn(
        provider: Arc<dyn LlmProvider>,
        tools: Arc<ToolRegistry>,
        spec: TaskSpec,
    ) -> Result<MonitoredHandle, anyhow::Error> {
        let (progress_tx, progress_rx) = mpsc::channel(64);
        let (interventor_tx, interventor_rx) = mpsc::channel(16);

        let config = crate::session::config::AgentConfig {
            model: spec
                .model
                .clone()
                .unwrap_or_else(|| provider.default_model().to_string()),
            max_iterations: spec.max_turns.unwrap_or(10),
            ..Default::default()
        };
        let kernel_config = config.to_kernel_config();

        let steppable = SteppableExecutor::new(provider, tools, kernel_config);

        let handle = tokio::spawn(async move {
            let mut runner =
                MonitoredRunner::new(spec, steppable, progress_tx, interventor_rx);
            match runner.run().await {
                Ok(result) => result,
                Err(e) => {
                    let _ = runner
                        .progress
                        .send(ProgressUpdate::Error {
                            message: e.to_string(),
                        })
                        .await;
                    runner.final_result_with_error(&e.to_string())
                }
            }
        });

        Ok(MonitoredHandle {
            handle,
            interventor: interventor_tx,
            progress: progress_rx,
        })
    }
}

// ── MonitoredRunner ──────────────────────────────────────────────

struct MonitoredRunner {
    spec: TaskSpec,
    steppable: SteppableExecutor,
    messages: Vec<ChatMessage>,
    ledger: TokenLedger,
    progress: mpsc::Sender<ProgressUpdate>,
    intervention: mpsc::Receiver<Intervention>,
    max_turns: u32,
}

impl MonitoredRunner {
    fn new(
        spec: TaskSpec,
        steppable: SteppableExecutor,
        progress: mpsc::Sender<ProgressUpdate>,
        intervention: mpsc::Receiver<Intervention>,
    ) -> Self {
        let system = spec.system_prompt.clone().unwrap_or_default();
        let messages = if system.is_empty() {
            vec![ChatMessage::user(&spec.task)]
        } else {
            vec![ChatMessage::system(&system), ChatMessage::user(&spec.task)]
        };

        Self {
            spec,
            steppable,
            messages,
            ledger: TokenLedger::new(),
            progress,
            intervention,
            max_turns: 10,
        }
    }

    async fn run(&mut self) -> Result<SubagentResult, anyhow::Error> {
        for turn in 1..=self.max_turns {
            // Check for interventions (non-blocking)
            while let Ok(i) = self.intervention.try_recv() {
                match i {
                    Intervention::Abort => {
                        info!("[Monitored {}] Abort requested", self.spec.id);
                        let result = self.final_result();
                        let _ = self
                            .progress
                            .send(ProgressUpdate::Done {
                                result: result.response.content.clone(),
                            })
                            .await;
                        return Ok(result);
                    }
                    Intervention::AddKeyInfo(info) => {
                        self.messages.push(ChatMessage::system(format!(
                            "[Key Info] {}",
                            info
                        )));
                    }
                    Intervention::AppendPrompt(prompt) => {
                        self.messages.push(ChatMessage::user(prompt));
                    }
                    Intervention::ExtendTurns(n) => {
                        self.max_turns += n;
                    }
                }
            }

            let _ = self
                .progress
                .send(ProgressUpdate::Thinking { turn: turn as usize })
                .await;

            let result = self
                .steppable
                .step(&mut self.messages, &mut self.ledger, None)
                .await
                .map_err(|e| anyhow::anyhow!("Step failed: {}", e))?;

            self.emit_tool_progress(&result).await;

            let summary: String = result
                .response
                .content
                .clone()
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            let _ = self
                .progress
                .send(ProgressUpdate::TurnComplete {
                    turn: turn as usize,
                    summary,
                })
                .await;

            if !result.should_continue {
                let final_result = self.final_result();
                let _ = self
                    .progress
                    .send(ProgressUpdate::Done {
                        result: final_result.response.content.clone(),
                    })
                    .await;
                return Ok(final_result);
            }
        }

        let final_result = self.final_result();
        let _ = self
            .progress
            .send(ProgressUpdate::Done {
                result: final_result.response.content.clone(),
            })
            .await;
        Ok(final_result)
    }

    async fn emit_tool_progress(&self, result: &StepResult) {
        for tr in &result.tool_results {
            let _ = self
                .progress
                .send(ProgressUpdate::ToolStart {
                    name: tr.tool_name.clone(),
                })
                .await;
            let _ = self
                .progress
                .send(ProgressUpdate::ToolResult {
                    name: tr.tool_name.clone(),
                    output: tr.output.clone(),
                })
                .await;
        }
    }

    fn final_result(&self) -> SubagentResult {
        let content = self
            .messages
            .last()
            .and_then(|m| m.content.clone())
            .unwrap_or_default();

        SubagentResult {
            id: self.spec.id.clone(),
            task: self.spec.task.clone(),
            response: AgentResponse {
                content,
                reasoning_content: None,
                tools_used: vec![],
                model: self.spec.model.clone(),
                token_usage: self.ledger.total_usage.clone(),
                cost: 0.0,
            },
            model: self.spec.model.clone(),
        }
    }

    fn final_result_with_error(&self, error_msg: &str) -> SubagentResult {
        let mut result = self.final_result();
        result.response.content = format!("Error: {}", error_msg);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_update_clone() {
        let p = ProgressUpdate::Thinking { turn: 1 };
        let _ = p.clone();
    }

    #[test]
    fn test_intervention_clone() {
        let i = Intervention::AddKeyInfo("test".to_string());
        let _ = i.clone();
    }
}
