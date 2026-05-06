//! Channel configuration schemas
//!
//! Configuration for various messaging channels (Telegram, Discord, Slack, etc.)

use crate::error::ChannelConfigError;
use serde::{Deserialize, Serialize};

/// Channels configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChannelsConfig {
    /// Telegram channel
    #[serde(default)]
    pub telegram: Option<TelegramConfig>,

    /// Discord channel
    #[serde(default)]
    pub discord: Option<DiscordConfig>,

    /// Slack channel
    #[serde(default)]
    pub slack: Option<SlackConfig>,

    /// Feishu channel
    #[serde(default)]
    pub feishu: Option<FeishuConfig>,

    /// WeChat channel
    #[serde(default)]
    pub wechat: Option<WechatConfig>,

    /// WebSocket channel
    #[serde(default)]
    pub websocket: Option<WebSocketConfig>,
}

// ── Telegram ─────────────────────────────────────────────────────────────

/// Telegram channel configuration
#[derive(Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// Enable this channel
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Bot token
    pub token: String,

    /// Allowed user IDs
    #[serde(default)]
    pub allow_from: Vec<String>,
}

impl std::fmt::Debug for TelegramConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramConfig")
            .field("enabled", &self.enabled)
            .field("token", &"***REDACTED***")
            .field("allow_from", &self.allow_from)
            .finish()
    }
}

// ── Discord ───────────────────────────────────────────────────────────────

/// Discord channel configuration
#[derive(Clone, Serialize, Deserialize)]
pub struct DiscordConfig {
    /// Enable this channel
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Bot token
    pub token: String,

    /// Allowed user IDs
    #[serde(default)]
    pub allow_from: Vec<String>,

    /// HTTP proxy URL for Discord REST API (e.g., "http://127.0.0.1:7890").
    /// Note: Discord Gateway WebSocket connections require a system-level
    /// transparent proxy (e.g., TUN mode) as tokio-tungstenite does not
    /// natively support HTTP proxies.
    #[serde(default, alias = "proxyUrl")]
    pub proxy_url: Option<String>,
}

impl std::fmt::Debug for DiscordConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscordConfig")
            .field("enabled", &self.enabled)
            .field("token", &"***REDACTED***")
            .field("allow_from", &self.allow_from)
            .field("proxy_url", &self.proxy_url)
            .finish()
    }
}

// ── Slack ─────────────────────────────────────────────────────────────────

/// Slack channel configuration
#[derive(Clone, Serialize, Deserialize)]
pub struct SlackConfig {
    /// Enable this channel
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Bot token (xoxb-...)
    pub bot_token: String,

    /// App token (xapp-...)
    pub app_token: String,

    /// Allowed user IDs
    #[serde(default, alias = "allowFrom")]
    pub allow_from: Vec<String>,

    /// Group policy: mention, open, or allowlist
    #[serde(default)]
    pub group_policy: Option<String>,
}

impl std::fmt::Debug for SlackConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlackConfig")
            .field("enabled", &self.enabled)
            .field("bot_token", &"***REDACTED***")
            .field("app_token", &"***REDACTED***")
            .field("allow_from", &self.allow_from)
            .field("group_policy", &self.group_policy)
            .finish()
    }
}

// ── Feishu ────────────────────────────────────────────────────────────────

/// Feishu channel configuration
#[derive(Clone, Serialize, Deserialize)]
pub struct FeishuConfig {
    /// Enable this channel
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// App ID
    #[serde(alias = "appId")]
    pub app_id: String,

    /// App Secret
    #[serde(alias = "appSecret")]
    pub app_secret: String,

    /// Verification token for webhook validation
    #[serde(default, alias = "verificationToken")]
    pub verification_token: Option<String>,

    /// Encrypt key for event decryption
    #[serde(default, alias = "encryptKey")]
    pub encrypt_key: Option<String>,

    /// Allowed users/groups (empty = allow all)
    #[serde(default, alias = "allowFrom")]
    pub allow_from: Vec<String>,
}

impl std::fmt::Debug for FeishuConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeishuConfig")
            .field("enabled", &self.enabled)
            .field("app_id", &self.app_id)
            .field("app_secret", &"***REDACTED***")
            .field(
                "verification_token",
                &self.verification_token.as_ref().map(|_| "***REDACTED***"),
            )
            .field(
                "encrypt_key",
                &self.encrypt_key.as_ref().map(|_| "***REDACTED***"),
            )
            .field("allow_from", &self.allow_from)
            .finish()
    }
}

// ── WeChat ────────────────────────────────────────────────────────────────

/// WeChat channel configuration
#[derive(Clone, Serialize, Deserialize)]
pub struct WechatConfig {
    /// Enable this channel
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Base URL for the iLink API (optional)
    #[serde(default, alias = "baseUrl")]
    pub base_url: Option<String>,

    /// Path to store credentials (optional)
    #[serde(default, alias = "credPath")]
    pub cred_path: Option<String>,

    /// Allowed users (empty = allow all)
    #[serde(default, alias = "allowFrom")]
    pub allow_from: Vec<String>,
}

impl std::fmt::Debug for WechatConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WechatConfig")
            .field("enabled", &self.enabled)
            .field("base_url", &self.base_url)
            .field("cred_path", &self.cred_path)
            .field("allow_from", &self.allow_from)
            .finish()
    }
}

// ── WebSocket ─────────────────────────────────────────────────────────────

/// WebSocket channel configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebSocketConfig {
    /// Enable this channel
    #[serde(default = "default_true")]
    pub enabled: bool,
}

// ── Default Functions ─────────────────────────────────────────────────────

fn default_true() -> bool {
    true
}

// ── Validation ────────────────────────────────────────────────────────────

impl ChannelsConfig {
    /// Validate all enabled channels
    pub fn validate(&self) -> Vec<ChannelConfigError> {
        Vec::new()
    }

    /// Count enabled channels
    pub fn enabled_count(&self) -> usize {
        let mut count = 0;
        if self.telegram.as_ref().is_some_and(|c| c.enabled) {
            count += 1;
        }
        if self.discord.as_ref().is_some_and(|c| c.enabled) {
            count += 1;
        }
        if self.slack.as_ref().is_some_and(|c| c.enabled) {
            count += 1;
        }
        if self.feishu.as_ref().is_some_and(|c| c.enabled) {
            count += 1;
        }
        if self.wechat.as_ref().is_some_and(|c| c.enabled) {
            count += 1;
        }
        if self.websocket.as_ref().is_some_and(|c| c.enabled) {
            count += 1;
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channels_config_enabled_count() {
        let yaml = r#"
telegram:
  enabled: true
  token: "test"
discord:
  enabled: false
  token: "test"
"#;
        let channels: ChannelsConfig = serde_yaml::from_str(yaml).unwrap();
        // Telegram (enabled) = 1
        assert_eq!(channels.enabled_count(), 1);
    }
}
