//! Channel factory pattern for unified channel creation and lifecycle management.
//!
//! Each channel type implements [`ChannelFactory`] to encapsulate:
//! - Configuration extraction from [`ChannelsConfig`]
//! - Vault secret resolution
//! - Channel instance creation
//!
//! [`ChannelRegistry`] collects all enabled factories and provides [`spawn_all()`]
//! to start them in parallel with unified error handling.

use colored::Colorize;

use gasket_engine::channels::{Channel, InboundSender};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A channel ready to be spawned as a background task.
pub struct SpawnableChannel {
    pub name: String,
    pub channel: Box<dyn Channel>,
}

/// Factory trait for creating channel instances.
///
/// Each channel type implements this to encapsulate its specific
/// config resolution, secret decryption, and instantiation logic.
pub trait ChannelFactory: Send + Sync {
    /// Display name (e.g., "Telegram").
    fn name(&self) -> &str;

    /// Create a channel instance.
    fn create(&self, inbound: InboundSender) -> Result<SpawnableChannel, String>;
}

// ---------------------------------------------------------------------------
// Secret resolution
// ---------------------------------------------------------------------------

/// Resolve a secret string through vault (JIT).
/// Returns the original string if no vault is available or no placeholders found.
#[allow(dead_code)]
fn resolve_secret(raw: &str, vault: Option<&gasket_engine::vault::VaultStore>) -> String {
    match vault {
        Some(v) => v.resolve_text(raw).unwrap_or_else(|e| {
            tracing::warn!("Failed to resolve vault placeholder: {}. Using raw value.", e);
            raw.to_string()
        }),
        None => raw.to_string(),
    }
}

/// Resolve an optional secret (only resolves if Some and non-empty).
#[allow(dead_code)]
fn resolve_optional_secret(
    raw: Option<&String>,
    vault: Option<&gasket_engine::vault::VaultStore>,
) -> Option<String> {
    raw.filter(|s| !s.is_empty()).map(|s| resolve_secret(s, vault))
}

// ---------------------------------------------------------------------------
// Telegram
// ---------------------------------------------------------------------------

#[cfg(feature = "telegram")]
pub struct TelegramFactory {
    token: String,
    allow_from: Vec<String>,
}

#[cfg(feature = "telegram")]
impl TelegramFactory {
    pub fn new(
        config: &gasket_engine::config::TelegramConfig,
        vault: Option<&gasket_engine::vault::VaultStore>,
    ) -> Self {
        Self {
            token: resolve_secret(&config.token, vault),
            allow_from: config.allow_from.clone(),
        }
    }
}

#[cfg(feature = "telegram")]
impl ChannelFactory for TelegramFactory {
    fn name(&self) -> &str {
        "Telegram"
    }

    fn create(&self, inbound: InboundSender) -> Result<SpawnableChannel, String> {
        let cfg = gasket_engine::channels::telegram::TelegramConfig {
            token: self.token.clone(),
            allow_from: self.allow_from.clone(),
        };
        Ok(SpawnableChannel {
            name: self.name().into(),
            channel: Box::new(
                gasket_engine::channels::telegram::TelegramChannel::new(cfg, inbound),
            ),
        })
    }
}

// ---------------------------------------------------------------------------
// Discord
// ---------------------------------------------------------------------------

#[cfg(feature = "discord")]
pub struct DiscordFactory {
    token: String,
    allow_from: Vec<String>,
}

#[cfg(feature = "discord")]
impl DiscordFactory {
    pub fn new(
        config: &gasket_engine::config::DiscordConfig,
        vault: Option<&gasket_engine::vault::VaultStore>,
    ) -> Self {
        Self {
            token: resolve_secret(&config.token, vault),
            allow_from: config.allow_from.clone(),
        }
    }
}

#[cfg(feature = "discord")]
impl ChannelFactory for DiscordFactory {
    fn name(&self) -> &str {
        "Discord"
    }

    fn create(&self, inbound: InboundSender) -> Result<SpawnableChannel, String> {
        let cfg = gasket_engine::channels::discord::DiscordConfig {
            token: self.token.clone(),
            allow_from: self.allow_from.clone(),
        };
        Ok(SpawnableChannel {
            name: self.name().into(),
            channel: Box::new(
                gasket_engine::channels::discord::DiscordChannel::new(cfg, inbound),
            ),
        })
    }
}

// ---------------------------------------------------------------------------
// Slack
// ---------------------------------------------------------------------------

#[cfg(feature = "slack")]
pub struct SlackFactory {
    bot_token: String,
    app_token: String,
    group_policy: Option<String>,
    allow_from: Vec<String>,
}

#[cfg(feature = "slack")]
impl SlackFactory {
    pub fn new(
        config: &gasket_engine::config::SlackConfig,
        vault: Option<&gasket_engine::vault::VaultStore>,
    ) -> Self {
        Self {
            bot_token: resolve_secret(&config.bot_token, vault),
            app_token: resolve_secret(&config.app_token, vault),
            group_policy: config.group_policy.clone(),
            allow_from: config.allow_from.clone(),
        }
    }
}

#[cfg(feature = "slack")]
impl ChannelFactory for SlackFactory {
    fn name(&self) -> &str {
        "Slack"
    }

    fn create(&self, inbound: InboundSender) -> Result<SpawnableChannel, String> {
        let cfg = gasket_engine::channels::slack::SlackConfig {
            bot_token: self.bot_token.clone(),
            app_token: self.app_token.clone(),
            group_policy: self.group_policy.clone(),
            allow_from: self.allow_from.clone(),
        };
        Ok(SpawnableChannel {
            name: self.name().into(),
            channel: Box::new(gasket_engine::channels::slack::SlackChannel::new(cfg, inbound)),
        })
    }
}

// ---------------------------------------------------------------------------
// Feishu
// ---------------------------------------------------------------------------

#[cfg(feature = "feishu")]
pub struct FeishuFactory {
    app_id: String,
    app_secret: String,
    verification_token: Option<String>,
    encrypt_key: Option<String>,
    allow_from: Vec<String>,
}

#[cfg(feature = "feishu")]
impl FeishuFactory {
    pub fn new(
        config: &gasket_engine::config::FeishuConfig,
        vault: Option<&gasket_engine::vault::VaultStore>,
    ) -> Self {
        Self {
            app_id: resolve_secret(&config.app_id, vault),
            app_secret: resolve_secret(&config.app_secret, vault),
            verification_token: resolve_optional_secret(config.verification_token.as_ref(), vault),
            encrypt_key: resolve_optional_secret(config.encrypt_key.as_ref(), vault),
            allow_from: config.allow_from.clone(),
        }
    }
}

#[cfg(feature = "feishu")]
impl ChannelFactory for FeishuFactory {
    fn name(&self) -> &str {
        "Feishu"
    }

    fn create(&self, inbound: InboundSender) -> Result<SpawnableChannel, String> {
        let cfg = gasket_engine::channels::feishu::FeishuConfig {
            app_id: self.app_id.clone(),
            app_secret: self.app_secret.clone(),
            verification_token: self.verification_token.clone(),
            encrypt_key: self.encrypt_key.clone(),
            allow_from: self.allow_from.clone(),
        };
        Ok(SpawnableChannel {
            name: self.name().into(),
            channel: Box::new(gasket_engine::channels::feishu::FeishuChannel::new(cfg, inbound)),
        })
    }
}

// ---------------------------------------------------------------------------
// DingTalk
// ---------------------------------------------------------------------------

#[cfg(feature = "dingtalk")]
pub struct DingTalkFactory {
    webhook_url: String,
    secret: Option<String>,
    access_token: Option<String>,
    allow_from: Vec<String>,
}

#[cfg(feature = "dingtalk")]
impl DingTalkFactory {
    pub fn new(
        config: &gasket_engine::config::DingTalkConfig,
        vault: Option<&gasket_engine::vault::VaultStore>,
    ) -> Self {
        Self {
            webhook_url: resolve_secret(&config.webhook_url, vault),
            secret: resolve_optional_secret(config.secret.as_ref(), vault),
            access_token: resolve_optional_secret(config.access_token.as_ref(), vault),
            allow_from: config.allow_from.clone(),
        }
    }
}

#[cfg(feature = "dingtalk")]
impl ChannelFactory for DingTalkFactory {
    fn name(&self) -> &str {
        "DingTalk"
    }

    fn create(&self, inbound: InboundSender) -> Result<SpawnableChannel, String> {
        let cfg = gasket_engine::channels::dingtalk::DingTalkConfig {
            webhook_url: self.webhook_url.clone(),
            secret: self.secret.clone(),
            access_token: self.access_token.clone(),
            allow_from: self.allow_from.clone(),
        };
        Ok(SpawnableChannel {
            name: self.name().into(),
            channel: Box::new(
                gasket_engine::channels::dingtalk::DingTalkChannel::new(cfg, inbound),
            ),
        })
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Registry of enabled channel factories.
///
/// Created from config, collects factories for all enabled channels,
/// and provides [`spawn_all()`] to start them with unified error handling.
pub struct ChannelRegistry {
    factories: Vec<Box<dyn ChannelFactory>>,
}

impl ChannelRegistry {
    /// Build a registry from configuration, including only enabled channels.
    ///
    /// Resolves secrets through vault during construction, so factories
    /// hold fully resolved config ready for instantiation.
    #[allow(unused_variables)]
    pub fn from_config(
        config: &gasket_engine::config::ChannelsConfig,
        vault: Option<&gasket_engine::vault::VaultStore>,
    ) -> Self {
        #[allow(unused_mut)]
        let mut factories: Vec<Box<dyn ChannelFactory>> = Vec::new();

        #[cfg(feature = "telegram")]
        if let Some(ref cfg) = config.telegram {
            if cfg.enabled {
                factories.push(Box::new(TelegramFactory::new(cfg, vault)));
            }
        }

        #[cfg(feature = "discord")]
        if let Some(ref cfg) = config.discord {
            if cfg.enabled {
                factories.push(Box::new(DiscordFactory::new(cfg, vault)));
            }
        }

        #[cfg(feature = "slack")]
        if let Some(ref cfg) = config.slack {
            if cfg.enabled {
                factories.push(Box::new(SlackFactory::new(cfg, vault)));
            }
        }

        #[cfg(feature = "feishu")]
        if let Some(ref cfg) = config.feishu {
            if cfg.enabled {
                factories.push(Box::new(FeishuFactory::new(cfg, vault)));
            }
        }

        #[cfg(feature = "dingtalk")]
        if let Some(ref cfg) = config.dingtalk {
            if cfg.enabled {
                factories.push(Box::new(DingTalkFactory::new(cfg, vault)));
            }
        }

        Self { factories }
    }

    /// Spawn all registered channels as background tasks.
    ///
    /// Returns `(task_handles, error_messages)`.
    pub fn spawn_all(
        self,
        inbound: &InboundSender,
    ) -> (Vec<tokio::task::JoinHandle<()>>, Vec<String>) {
        let mut tasks = Vec::new();
        let mut errors = Vec::new();

        for factory in self.factories {
            let name = factory.name().to_string();
            match factory.create(inbound.clone()) {
                Ok(spawnable) => {
                    println!("{} {} channel", "✓".green(), spawnable.name);
                    tasks.push(tokio::spawn(async move {
                        let mut ch = spawnable.channel;
                        if let Err(e) = ch.start().await {
                            tracing::error!("{} channel error: {}", spawnable.name, e);
                        }
                    }));
                }
                Err(e) => {
                    errors.push(format!("{}: {}", name, e));
                }
            }
        }

        if !errors.is_empty() {
            tracing::warn!("{} channel(s) failed to initialize", errors.len());
        }

        (tasks, errors)
    }
}
