//! Subagent manager for background task execution

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, instrument, warn};

use crate::agent::prompt;
use crate::bus::events::SessionKey;
use crate::providers::LlmProvider;
use crate::tools::ToolRegistry;

use super::loop_::{AgentConfig, AgentLoop, AgentResponse};

/// Default timeout for subagent execution (10 minutes)
const SUBAGENT_TIMEOUT_SECS: u64 = 600;

pub struct SubagentManager {
    provider: Arc<dyn LlmProvider>,
    workspace: PathBuf,
    tool_factory: Arc<dyn Fn() -> ToolRegistry + Send + Sync>,
    outbound_tx: mpsc::Sender<crate::bus::events::OutboundMessage>,
}

impl SubagentManager {
    pub async fn new(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        tool_factory: Arc<dyn Fn() -> ToolRegistry + Send + Sync>,
        outbound_tx: mpsc::Sender<crate::bus::events::OutboundMessage>,
    ) -> Self {
        Self {
            provider,
            workspace,
            tool_factory,
            outbound_tx,
        }
    }

    #[instrument(name = "subagent.submit", skip_all)]
    pub fn submit(&self, prompt: &str, channel: &str, chat_id: &str) -> anyhow::Result<()> {
        let provider = self.provider.clone();
        let workspace = self.workspace.clone();
        let tool_factory = self.tool_factory.clone();
        let outbound_tx = self.outbound_tx.clone();
        let prompt = prompt.to_string();

        let channel_enum = match channel {
            "telegram" => crate::bus::ChannelType::Telegram,
            "discord" => crate::bus::ChannelType::Discord,
            "slack" => crate::bus::ChannelType::Slack,
            "email" => crate::bus::ChannelType::Email,
            "dingtalk" => crate::bus::ChannelType::Dingtalk,
            "feishu" => crate::bus::ChannelType::Feishu,
            _ => crate::bus::ChannelType::Cli,
        };
        let chat_id = chat_id.to_string();
        let session_key = SessionKey::new(channel_enum.clone(), &chat_id);

        tokio::spawn(async move {
            info!("Subagent task started: {}", &prompt[..prompt.len().min(80)]);
            let agent_config = AgentConfig {
                model: provider.default_model().to_string(),
                max_iterations: 10,
                ..Default::default()
            };
            let tools = tool_factory();

            let mut agent =
                match AgentLoop::builder(provider, workspace.clone(), agent_config, tools) {
                    Ok(a) => a,
                    Err(e) => {
                        warn!("Failed to initialise subagent: {}", e);
                        return;
                    }
                };

            // Load minimal system prompt directly (no hook dispatch)
            let system_prompt =
                match prompt::load_system_prompt(&workspace, prompt::BOOTSTRAP_FILES_MINIMAL).await
                {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("Failed to load minimal system prompt: {}", e);
                        return;
                    }
                };
            agent.set_system_prompt(system_prompt);

            let timeout_duration = std::time::Duration::from_secs(SUBAGENT_TIMEOUT_SECS);
            let result = tokio::time::timeout(
                timeout_duration,
                agent.process_direct(&prompt, &session_key),
            )
            .await;

            let content = match result {
                Ok(Ok(response)) => format!("Background task completed:\n{}", response.content),
                Ok(Err(e)) => format!("Background task failed: {}", e),
                Err(_) => format!(
                    "Background task failed: Execution timed out after {:?}",
                    timeout_duration
                ),
            };

            let msg = crate::bus::events::OutboundMessage {
                channel: channel_enum,
                chat_id,
                content,
                metadata: None,
                trace_id: None,
            };

            // Route through the Outbound Actor — no direct HTTP call
            if let Err(e) = outbound_tx.send(msg).await {
                warn!("Failed to send subagent result to outbound channel: {}", e);
            }
        });

        Ok(())
    }

    /// Submit a prompt and **synchronously wait** for the agent response.
    ///
    /// Unlike `submit()` (fire-and-forget), this method blocks the caller
    /// until the subagent finishes. It is designed for governance-layer
    /// agents (e.g. review gates) where the pipeline must wait for a
    /// decision before proceeding.
    ///
    /// An optional `system_prompt` can be provided to inject a role-specific
    /// SOUL.md — if `None`, the default minimal bootstrap prompt is used.
    #[instrument(name = "subagent.submit_and_wait", skip_all)]
    pub async fn submit_and_wait(
        &self,
        prompt_text: &str,
        system_prompt: Option<&str>,
    ) -> anyhow::Result<AgentResponse> {
        let provider = self.provider.clone();
        let workspace = self.workspace.clone();
        let tool_factory = self.tool_factory.clone();
        let prompt_text = prompt_text.to_string();
        let custom_system = system_prompt.map(|s| s.to_string());

        let (tx, rx) = tokio::sync::oneshot::channel::<anyhow::Result<AgentResponse>>();

        tokio::spawn(async move {
            let result = async {
                info!(
                    "Subagent (sync) started: {}",
                    &prompt_text[..prompt_text.len().min(80)]
                );
                let agent_config = AgentConfig {
                    model: provider.default_model().to_string(),
                    max_iterations: 10,
                    ..Default::default()
                };
                let tools = tool_factory();

                let mut agent =
                    AgentLoop::builder(provider, workspace.clone(), agent_config, tools)?;

                // Use custom system prompt if provided, otherwise load default
                let sys = match custom_system {
                    Some(s) => s,
                    None => {
                        prompt::load_system_prompt(&workspace, prompt::BOOTSTRAP_FILES_MINIMAL)
                            .await?
                    }
                };
                agent.set_system_prompt(sys);

                let session_key = SessionKey::new(crate::bus::ChannelType::Cli, "pipeline_sync");
                let timeout_duration = std::time::Duration::from_secs(SUBAGENT_TIMEOUT_SECS);

                tokio::time::timeout(
                    timeout_duration,
                    agent.process_direct(&prompt_text, &session_key),
                )
                .await
                .map_err(|_| anyhow::anyhow!("Subagent timed out after {SUBAGENT_TIMEOUT_SECS}s"))?
                .map_err(|e| anyhow::anyhow!("{}", e))
            }
            .await;

            // Send result back; if the receiver was dropped, discard silently
            let _ = tx.send(result);
        });

        rx.await
            .map_err(|_| anyhow::anyhow!("Subagent task was cancelled"))?
    }
}
