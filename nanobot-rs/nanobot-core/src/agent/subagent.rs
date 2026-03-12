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
                ws_message: None,
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
        info!(
            "Subagent (sync) started: {}",
            &prompt_text[..prompt_text.len().min(80)]
        );

        let agent_config = AgentConfig {
            model: self.provider.default_model().to_string(),
            max_iterations: 10,
            ..Default::default()
        };
        let tools = (self.tool_factory)();

        let mut agent = AgentLoop::builder(
            self.provider.clone(),
            self.workspace.clone(),
            agent_config,
            tools,
        )?;

        let sys = match system_prompt {
            Some(s) => s.to_string(),
            None => {
                prompt::load_system_prompt(&self.workspace, prompt::BOOTSTRAP_FILES_MINIMAL).await?
            }
        };
        agent.set_system_prompt(sys);

        let session_key = SessionKey::new(crate::bus::ChannelType::Cli, "pipeline_sync");

        tokio::time::timeout(
            std::time::Duration::from_secs(SUBAGENT_TIMEOUT_SECS),
            agent.process_direct(prompt_text, &session_key),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Subagent timed out after {SUBAGENT_TIMEOUT_SECS}s"))?
        .map_err(|e| anyhow::anyhow!("{}", e))
    }

    /// Submit a prompt with a **specific model** and wait for the response.
    ///
    /// This method allows switching to a different provider/model for the
    /// subagent execution. Used by the `switch_model` tool.
    ///
    /// # Arguments
    /// * `prompt_text` - The task description for the subagent
    /// * `system_prompt` - Optional custom system prompt (uses minimal bootstrap if None)
    /// * `provider` - The LLM provider to use for this execution
    /// * `agent_config` - Agent configuration including model, temperature, etc.
    #[instrument(name = "subagent.submit_and_wait_with_model", skip_all)]
    pub async fn submit_and_wait_with_model(
        &self,
        prompt_text: &str,
        system_prompt: Option<&str>,
        provider: Arc<dyn LlmProvider>,
        agent_config: AgentConfig,
    ) -> anyhow::Result<AgentResponse> {
        info!(
            "Subagent (model switch) started with model '{}': {}",
            agent_config.model,
            &prompt_text[..prompt_text.len().min(80)]
        );

        let tools = (self.tool_factory)();
        let mut agent = AgentLoop::builder(provider, self.workspace.clone(), agent_config, tools)?;

        let sys = match system_prompt {
            Some(s) => s.to_string(),
            None => {
                prompt::load_system_prompt(&self.workspace, prompt::BOOTSTRAP_FILES_MINIMAL).await?
            }
        };
        agent.set_system_prompt(sys);

        let session_key = SessionKey::new(crate::bus::ChannelType::Cli, "model_switch_sync");

        tokio::time::timeout(
            std::time::Duration::from_secs(SUBAGENT_TIMEOUT_SECS),
            agent.process_direct(prompt_text, &session_key),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Model switch task timed out after {SUBAGENT_TIMEOUT_SECS}s"))?
        .map_err(|e| anyhow::anyhow!("{}", e))
    }
}
