//! Subagent manager for background task execution

use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, instrument, warn};

use crate::config::ChannelsConfig;
use crate::hooks::prompt::BootstrapHook;
use crate::providers::LlmProvider;
use crate::tools::ToolRegistry;

use super::loop_::{AgentConfig, AgentLoop};

pub struct SubagentManager {
    provider: Arc<dyn LlmProvider>,
    workspace: PathBuf,
    tool_factory: Arc<dyn Fn() -> ToolRegistry + Send + Sync>,
    channels_config: Arc<ChannelsConfig>,
}

impl SubagentManager {
    pub async fn new(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        tool_factory: Arc<dyn Fn() -> ToolRegistry + Send + Sync>,
        channels_config: Arc<ChannelsConfig>,
    ) -> Self {
        Self {
            provider,
            workspace,
            tool_factory,
            channels_config,
        }
    }

    #[instrument(name = "subagent.submit", skip_all)]
    pub fn submit(&self, prompt: &str, channel: &str, chat_id: &str) -> anyhow::Result<()> {
        let provider = self.provider.clone();
        let workspace = self.workspace.clone();
        let tool_factory = self.tool_factory.clone();
        let channels_config = self.channels_config.clone();
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
        let session_key = format!("subagent:{}:{}", channel, chat_id);

        tokio::spawn(async move {
            info!("Subagent task started: {}", &prompt[..prompt.len().min(80)]);
            let agent_config = AgentConfig {
                model: provider.default_model().to_string(),
                max_iterations: 10,
                ..Default::default()
            };
            let tools = tool_factory();

            let mut agent =
                match AgentLoop::builder(provider, workspace.clone(), agent_config, tools).await {
                    Ok(a) => a,
                    Err(e) => {
                        warn!("Failed to initialise subagent: {}", e);
                        return;
                    }
                };

            // Register minimal bootstrap hook for subagents
            let hook = match BootstrapHook::new_minimal(&workspace).await {
                Ok(h) => h,
                Err(e) => {
                    warn!("Failed to load minimal bootstrap hook: {}", e);
                    return;
                }
            };
            agent.register_hook(Arc::new(hook));

            let result = agent.process_direct(&prompt, &session_key).await;

            let content = match result {
                Ok(response) => format!("Background task completed:\n{}", response.content),
                Err(e) => format!("Background task failed: {}", e),
            };

            let msg = crate::bus::events::OutboundMessage {
                channel: channel_enum,
                chat_id,
                content,
                metadata: None,
                trace_id: None,
            };

            if let Err(e) = crate::channels::send_outbound(&channels_config, msg).await {
                warn!("Failed to send subagent result: {}", e);
            }
        });

        Ok(())
    }
}
