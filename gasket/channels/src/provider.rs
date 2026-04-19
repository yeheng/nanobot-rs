//! IM provider enum and registry.
//!
//! Replaces `ChannelRegistry`, `ChannelFactory`, `OutboundSenderRegistry`, and
//! `OutboundSender` with a single compile-time enum.

use crate::adapter::ImAdapter;
use crate::events::{ChannelType, OutboundMessage};

/// A collection of enabled IM providers.
pub struct ImProviders {
    providers: Vec<ImProvider>,
}

/// Compile-time enum of all supported messaging platforms.
///
/// This eliminates the old `Box<dyn ChannelFactory>` and `Arc<dyn OutboundSender>`
/// indirections. Platforms are known at compile time (feature-gated), so an enum
/// is the simplest and fastest abstraction.
pub enum ImProvider {
    #[cfg(feature = "telegram")]
    Telegram(crate::telegram::TelegramAdapter),
    #[cfg(feature = "discord")]
    Discord(crate::discord::DiscordAdapter),
    #[cfg(feature = "slack")]
    Slack(crate::slack::SlackAdapter),
    #[cfg(feature = "websocket")]
    WebSocket(crate::websocket::WebSocketAdapter),
    Cli(crate::websocket::CliAdapter), // re-use the no-op adapter
    #[cfg(feature = "dingtalk")]
    DingTalk(crate::dingtalk::DingTalkAdapter),
    #[cfg(feature = "feishu")]
    Feishu(crate::feishu::FeishuAdapter),
    #[cfg(feature = "wecom")]
    Wecom(crate::wecom::WeComAdapter),
    Tui(crate::tui::TuiAdapter),
}

impl ImProvider {
    pub fn channel_type(&self) -> ChannelType {
        match self {
            #[cfg(feature = "telegram")]
            Self::Telegram(_) => ChannelType::Telegram,
            #[cfg(feature = "discord")]
            Self::Discord(_) => ChannelType::Discord,
            #[cfg(feature = "slack")]
            Self::Slack(_) => ChannelType::Slack,
            #[cfg(feature = "websocket")]
            Self::WebSocket(_) => ChannelType::WebSocket,
            Self::Cli(_) => ChannelType::Cli,
            #[cfg(feature = "dingtalk")]
            Self::DingTalk(_) => ChannelType::Dingtalk,
            #[cfg(feature = "feishu")]
            Self::Feishu(_) => ChannelType::Feishu,
            #[cfg(feature = "wecom")]
            Self::Wecom(_) => ChannelType::Wecom,
            Self::Tui(_) => ChannelType::Custom("tui".to_string()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            #[cfg(feature = "telegram")]
            Self::Telegram(a) => a.name(),
            #[cfg(feature = "discord")]
            Self::Discord(a) => a.name(),
            #[cfg(feature = "slack")]
            Self::Slack(a) => a.name(),
            #[cfg(feature = "websocket")]
            Self::WebSocket(a) => a.name(),
            Self::Cli(a) => a.name(),
            #[cfg(feature = "dingtalk")]
            Self::DingTalk(a) => a.name(),
            #[cfg(feature = "feishu")]
            Self::Feishu(a) => a.name(),
            #[cfg(feature = "wecom")]
            Self::Wecom(a) => a.name(),
            Self::Tui(a) => a.name(),
        }
    }

    pub async fn start(&self, inbound: crate::middleware::InboundSender) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "telegram")]
            Self::Telegram(a) => a.start(inbound).await,
            #[cfg(feature = "discord")]
            Self::Discord(a) => a.start(inbound).await,
            #[cfg(feature = "slack")]
            Self::Slack(a) => a.start(inbound).await,
            #[cfg(feature = "websocket")]
            Self::WebSocket(a) => a.start(inbound).await,
            Self::Cli(a) => a.start(inbound).await,
            #[cfg(feature = "dingtalk")]
            Self::DingTalk(a) => a.start(inbound).await,
            #[cfg(feature = "feishu")]
            Self::Feishu(a) => a.start(inbound).await,
            #[cfg(feature = "wecom")]
            Self::Wecom(a) => a.start(inbound).await,
            Self::Tui(a) => a.start(inbound).await,
        }
    }

    /// Return webhook routes for this provider, if any.
    pub fn routes(&self) -> Option<axum::Router> {
        match self {
            #[cfg(feature = "websocket")]
            Self::WebSocket(a) => Some(a.routes()),
            #[cfg(feature = "dingtalk")]
            Self::DingTalk(a) => Some(a.routes()),
            #[cfg(feature = "feishu")]
            Self::Feishu(a) => Some(a.routes()),
            #[cfg(feature = "wecom")]
            Self::Wecom(a) => Some(a.routes()),
            _ => None,
        }
    }

    pub async fn send(&self, msg: &OutboundMessage) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "telegram")]
            Self::Telegram(a) => a.send(msg).await,
            #[cfg(feature = "discord")]
            Self::Discord(a) => a.send(msg).await,
            #[cfg(feature = "slack")]
            Self::Slack(a) => a.send(msg).await,
            #[cfg(feature = "websocket")]
            Self::WebSocket(a) => a.send(msg).await,
            Self::Cli(a) => a.send(msg).await,
            #[cfg(feature = "dingtalk")]
            Self::DingTalk(a) => a.send(msg).await,
            #[cfg(feature = "feishu")]
            Self::Feishu(a) => a.send(msg).await,
            #[cfg(feature = "wecom")]
            Self::Wecom(a) => a.send(msg).await,
            Self::Tui(a) => a.send(msg).await,
        }
    }
}

impl ImProviders {
    /// Build providers from configuration, including only enabled platforms.
    pub fn from_config(
        config: &crate::config::ChannelsConfig,
        inbound: crate::middleware::InboundSender,
    ) -> Self {
        let mut providers = Vec::new();

        #[cfg(feature = "telegram")]
        if let Some(ref cfg) = config.telegram {
            if cfg.enabled {
                providers.push(ImProvider::Telegram(
                    crate::telegram::TelegramAdapter::from_config(cfg, inbound.clone()),
                ));
            }
        }

        #[cfg(feature = "discord")]
        if let Some(ref cfg) = config.discord {
            if cfg.enabled {
                providers.push(ImProvider::Discord(
                    crate::discord::DiscordAdapter::from_config(cfg, inbound.clone()),
                ));
            }
        }

        #[cfg(feature = "slack")]
        if let Some(ref cfg) = config.slack {
            if cfg.enabled {
                providers.push(ImProvider::Slack(crate::slack::SlackAdapter::from_config(
                    cfg,
                    inbound.clone(),
                )));
            }
        }

        #[cfg(feature = "dingtalk")]
        if let Some(ref cfg) = config.dingtalk {
            if cfg.enabled {
                providers.push(ImProvider::DingTalk(
                    crate::dingtalk::DingTalkAdapter::from_config(cfg, inbound.clone()),
                ));
            }
        }

        #[cfg(feature = "feishu")]
        if let Some(ref cfg) = config.feishu {
            if cfg.enabled {
                providers.push(ImProvider::Feishu(
                    crate::feishu::FeishuAdapter::from_config(cfg, inbound.clone()),
                ));
            }
        }

        #[cfg(feature = "wecom")]
        if let Some(ref cfg) = config.wecom {
            if cfg.enabled {
                providers.push(ImProvider::Wecom(crate::wecom::WeComAdapter::from_config(
                    cfg,
                    inbound.clone(),
                )));
            }
        }

        #[cfg(feature = "websocket")]
        if let Some(ref cfg) = config.websocket {
            if cfg.enabled {
                providers.push(ImProvider::WebSocket(
                    crate::websocket::WebSocketAdapter::from_config(cfg, inbound.clone()),
                ));
            }
        }

        // Always register the no-op CLI adapter so outbound messages tagged with
        // ChannelType::Cli are gracefully absorbed instead of dropped with a warning.
        providers.push(ImProvider::Cli(crate::websocket::CliAdapter));

        // Register TUI adapter if enabled
        if config.tui.as_ref().is_some_and(|c| c.enabled) {
            providers.push(ImProvider::Tui(crate::tui::TuiAdapter::from_config(
                config.tui.as_ref().unwrap(),
            )));
        }

        Self { providers }
    }

    pub fn push(&mut self, provider: ImProvider) {
        self.providers.push(provider);
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    pub fn len(&self) -> usize {
        self.providers.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &ImProvider> {
        self.providers.iter()
    }

    /// Spawn all providers that have an inbound loop.
    pub fn spawn_all(
        &self,
        inbound: &crate::middleware::InboundSender,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        let mut tasks = Vec::new();
        for provider in &self.providers {
            let name = provider.name().to_string();
            let inbound = inbound.clone();
            let provider_clone = match provider {
                #[cfg(feature = "telegram")]
                ImProvider::Telegram(a) => ImProvider::Telegram(a.clone()),
                #[cfg(feature = "discord")]
                ImProvider::Discord(a) => ImProvider::Discord(a.clone()),
                #[cfg(feature = "slack")]
                ImProvider::Slack(a) => ImProvider::Slack(a.clone()),
                #[cfg(feature = "websocket")]
                ImProvider::WebSocket(a) => ImProvider::WebSocket(a.clone()),
                ImProvider::Cli(a) => ImProvider::Cli(*a),
                ImProvider::Tui(a) => ImProvider::Tui(a.clone()),
                #[cfg(feature = "dingtalk")]
                ImProvider::DingTalk(a) => ImProvider::DingTalk(a.clone()),
                #[cfg(feature = "feishu")]
                ImProvider::Feishu(a) => ImProvider::Feishu(a.clone()),
                #[cfg(feature = "wecom")]
                ImProvider::Wecom(a) => ImProvider::Wecom(a.clone()),
            };
            tasks.push(tokio::spawn(async move {
                if let Err(e) = provider_clone.start(inbound).await {
                    tracing::error!("{} adapter start error: {}", name, e);
                }
            }));
        }
        tasks
    }

    /// Send an outbound message by matching its channel type.
    pub async fn send(&self, msg: &OutboundMessage) -> anyhow::Result<()> {
        for provider in &self.providers {
            if provider.channel_type() == msg.channel {
                return provider.send(msg).await;
            }
        }
        tracing::warn!(
            "No provider found for channel {:?}, dropping message",
            msg.channel
        );
        Ok(())
    }
}
