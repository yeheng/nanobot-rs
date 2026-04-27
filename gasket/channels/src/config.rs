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

    /// DingTalk channel
    #[serde(default)]
    pub dingtalk: Option<DingTalkConfig>,

    /// WeCom channel
    #[serde(default)]
    pub wecom: Option<WeComConfig>,

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

// ── DingTalk ──────────────────────────────────────────────────────────────

/// DingTalk channel configuration
#[derive(Clone, Serialize, Deserialize)]
pub struct DingTalkConfig {
    /// Enable this channel
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Webhook URL (for outgoing messages)
    #[serde(default, alias = "webhookUrl")]
    pub webhook_url: String,

    /// Secret key for signing (optional but recommended)
    #[serde(default)]
    pub secret: Option<String>,

    /// Access token (alternative to webhook_url)
    #[serde(default, alias = "accessToken")]
    pub access_token: Option<String>,

    /// Allowed users (empty = allow all)
    #[serde(default, alias = "allowFrom")]
    pub allow_from: Vec<String>,
}

impl std::fmt::Debug for DingTalkConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DingTalkConfig")
            .field("enabled", &self.enabled)
            .field("webhook_url", &"***REDACTED***")
            .field("secret", &self.secret.as_ref().map(|_| "***REDACTED***"))
            .field(
                "access_token",
                &self.access_token.as_ref().map(|_| "***REDACTED***"),
            )
            .field("allow_from", &self.allow_from)
            .finish()
    }
}

// ── WeCom ─────────────────────────────────────────────────────────────────

/// WeCom channel configuration
#[derive(Clone, Serialize, Deserialize)]
pub struct WeComConfig {
    /// Enable this channel
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Corp ID
    pub corpid: String,

    /// Corp Secret
    pub corpsecret: String,

    /// Agent ID for the bot application
    #[serde(alias = "agentId")]
    pub agent_id: i64,

    /// Token for callback verification (optional)
    #[serde(default, alias = "token")]
    pub token: Option<String>,

    /// EncodingAESKey for callback message encryption/decryption (optional, 43 chars)
    #[serde(default, alias = "encodingAesKey")]
    pub encoding_aes_key: Option<String>,

    /// Allowed users (empty = allow all)
    #[serde(default, alias = "allowFrom")]
    pub allow_from: Vec<String>,
}

impl std::fmt::Debug for WeComConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WeComConfig")
            .field("enabled", &self.enabled)
            .field("corpid", &self.corpid)
            .field("corpsecret", &"***REDACTED***")
            .field("agent_id", &self.agent_id)
            .field("token", &self.token.as_ref().map(|_| "***REDACTED***"))
            .field(
                "encoding_aes_key",
                &self.encoding_aes_key.as_ref().map(|_| "***REDACTED***"),
            )
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
        let mut errors = Vec::new();

        // Validate DingTalk configuration
        if let Some(ref dingtalk) = self.dingtalk {
            if dingtalk.enabled {
                // DingTalk requires either webhook_url or access_token
                if dingtalk.webhook_url.is_empty() && dingtalk.access_token.is_none() {
                    errors.push(ChannelConfigError::InvalidChannelConfig(
                        "dingtalk".to_string(),
                        "requires either webhook_url or access_token".to_string(),
                    ));
                }
            }
        }

        errors
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
        if self.dingtalk.as_ref().is_some_and(|c| c.enabled) {
            count += 1;
        }
        if self.wecom.as_ref().is_some_and(|c| c.enabled) {
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
    fn test_dingtalk_config_parsing() {
        let yaml = r#"
enabled: true
webhookUrl: https://oapi.dingtalk.com/robot/send?access_token=xxx
secret: SECxxx
"#;
        let dingtalk: DingTalkConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(dingtalk.enabled);
        assert!(!dingtalk.webhook_url.is_empty());
        assert!(dingtalk.secret.is_some());
    }

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

    #[test]
    fn test_channels_validate_dingtalk() {
        let yaml = r#"
dingtalk:
  enabled: true
"#;
        let channels: ChannelsConfig = serde_yaml::from_str(yaml).unwrap();
        let errors = channels.validate();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            ChannelConfigError::InvalidChannelConfig(_, _)
        ));
    }
}
