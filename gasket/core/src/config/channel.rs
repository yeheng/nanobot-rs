//! Channel configuration schemas
//!
//! Configuration for various messaging channels (Telegram, Discord, Slack, etc.)

use crate::error::ConfigValidationError;
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

    /// Email channel
    #[serde(default)]
    pub email: Option<EmailConfig>,

    /// DingTalk channel
    #[serde(default)]
    pub dingtalk: Option<DingTalkConfig>,
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
}

impl std::fmt::Debug for DiscordConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscordConfig")
            .field("enabled", &self.enabled)
            .field("token", &"***REDACTED***")
            .field("allow_from", &self.allow_from)
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

// ── Email ─────────────────────────────────────────────────────────────────

/// Email channel configuration
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct EmailConfig {
    /// Enable this channel
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// IMAP server host
    #[serde(default, alias = "imapHost")]
    pub imap_host: Option<String>,

    /// IMAP server port (default: 993)
    #[serde(default = "default_imap_port", alias = "imapPort")]
    pub imap_port: u16,

    /// IMAP username
    #[serde(default, alias = "imapUsername")]
    pub imap_username: Option<String>,

    /// IMAP password
    #[serde(default, alias = "imapPassword")]
    pub imap_password: Option<String>,

    /// SMTP server host
    #[serde(default, alias = "smtpHost")]
    pub smtp_host: Option<String>,

    /// SMTP server port (default: 587)
    #[serde(default = "default_smtp_port", alias = "smtpPort")]
    pub smtp_port: u16,

    /// SMTP username
    #[serde(default, alias = "smtpUsername")]
    pub smtp_username: Option<String>,

    /// SMTP password
    #[serde(default, alias = "smtpPassword")]
    pub smtp_password: Option<String>,

    /// From email address
    #[serde(default, alias = "fromAddress")]
    pub from_address: Option<String>,

    /// Allowed senders (empty = allow all)
    #[serde(default, alias = "allowFrom")]
    pub allow_from: Vec<String>,

    /// User consent for email access
    #[serde(default)]
    pub consent_granted: bool,
}

impl std::fmt::Debug for EmailConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmailConfig")
            .field("enabled", &self.enabled)
            .field("imap_host", &self.imap_host)
            .field("imap_port", &self.imap_port)
            .field("imap_username", &self.imap_username)
            .field(
                "imap_password",
                &self.imap_password.as_ref().map(|_| "***REDACTED***"),
            )
            .field("smtp_host", &self.smtp_host)
            .field("smtp_port", &self.smtp_port)
            .field("smtp_username", &self.smtp_username)
            .field(
                "smtp_password",
                &self.smtp_password.as_ref().map(|_| "***REDACTED***"),
            )
            .field("from_address", &self.from_address)
            .field("allow_from", &self.allow_from)
            .field("consent_granted", &self.consent_granted)
            .finish()
    }
}

impl EmailConfig {
    /// Check if IMAP configuration is complete
    pub fn has_imap_config(&self) -> bool {
        self.imap_host.is_some() && self.imap_username.is_some() && self.imap_password.is_some()
    }

    /// Check if SMTP configuration is complete
    pub fn has_smtp_config(&self) -> bool {
        self.smtp_host.is_some()
            && self.smtp_username.is_some()
            && self.smtp_password.is_some()
            && self.from_address.is_some()
    }

    /// Check if email has either valid IMAP or SMTP configuration
    pub fn has_valid_config(&self) -> bool {
        self.has_imap_config() || self.has_smtp_config()
    }

    /// Build email config, returning error if validation fails
    #[cfg(feature = "email")]
    pub fn build_or_err(
        &self,
    ) -> Result<crate::channels::email::EmailConfig, ConfigValidationError> {
        if !self.has_valid_config() {
            return Err(ConfigValidationError::IncompleteEmailConfig);
        }

        // Build with proper error messages for missing fields
        Ok(crate::channels::email::EmailConfig {
            imap_host: self.imap_host.clone().unwrap_or_default(),
            imap_port: self.imap_port,
            imap_username: self.imap_username.clone().unwrap_or_default(),
            imap_password: self.imap_password.clone().unwrap_or_default(),
            smtp_host: self.smtp_host.clone().unwrap_or_default(),
            smtp_port: self.smtp_port,
            smtp_username: self.smtp_username.clone().unwrap_or_default(),
            smtp_password: self.smtp_password.clone().unwrap_or_default(),
            from_address: self.from_address.clone().unwrap_or_default(),
            allow_from: self.allow_from.clone(),
            consent_granted: self.consent_granted,
        })
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

// ── Default Functions ─────────────────────────────────────────────────────

fn default_imap_port() -> u16 {
    993
}

fn default_smtp_port() -> u16 {
    587
}

fn default_true() -> bool {
    true
}

// ── Validation ────────────────────────────────────────────────────────────

impl ChannelsConfig {
    /// Validate all enabled channels
    pub fn validate(&self) -> Vec<ConfigValidationError> {
        let mut errors = Vec::new();

        if let Some(ref email) = self.email {
            if email.enabled && !email.has_valid_config() {
                errors.push(ConfigValidationError::IncompleteEmailConfig);
            }
        }

        // Validate DingTalk configuration
        if let Some(ref dingtalk) = self.dingtalk {
            if dingtalk.enabled {
                // DingTalk requires either webhook_url or access_token
                if dingtalk.webhook_url.is_empty() && dingtalk.access_token.is_none() {
                    errors.push(ConfigValidationError::InvalidChannelConfig(
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
        if self.email.as_ref().is_some_and(|c| c.enabled) {
            count += 1;
        }
        if self.dingtalk.as_ref().is_some_and(|c| c.enabled) {
            count += 1;
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_email_config_has_valid_config() {
        // Valid IMAP config
        let email_imap = EmailConfig {
            enabled: true,
            imap_host: Some("imap.example.com".to_string()),
            imap_username: Some("user@example.com".to_string()),
            imap_password: Some("password".to_string()),
            ..Default::default()
        };
        assert!(email_imap.has_valid_config());
        assert!(email_imap.has_imap_config());
        assert!(!email_imap.has_smtp_config());

        // Valid SMTP config
        let email_smtp = EmailConfig {
            enabled: true,
            smtp_host: Some("smtp.example.com".to_string()),
            smtp_username: Some("user@example.com".to_string()),
            smtp_password: Some("password".to_string()),
            from_address: Some("user@example.com".to_string()),
            ..Default::default()
        };
        assert!(email_smtp.has_valid_config());
        assert!(email_smtp.has_smtp_config());
        assert!(!email_smtp.has_imap_config());

        // Invalid config (missing fields)
        let email_invalid = EmailConfig {
            enabled: true,
            imap_host: Some("imap.example.com".to_string()),
            // Missing username and password
            ..Default::default()
        };
        assert!(!email_invalid.has_valid_config());
    }

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
email:
  enabled: true
  imapHost: imap.example.com
  imapUsername: user
  imapPassword: pass
"#;
        let channels: ChannelsConfig = serde_yaml::from_str(yaml).unwrap();
        // Telegram (enabled) + Email (enabled) = 2
        assert_eq!(channels.enabled_count(), 2);
    }

    #[test]
    fn test_channels_validate_email() {
        let yaml = r#"
email:
  enabled: true
  imapHost: imap.example.com
"#;
        let channels: ChannelsConfig = serde_yaml::from_str(yaml).unwrap();
        let errors = channels.validate();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            ConfigValidationError::IncompleteEmailConfig
        ));
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
            ConfigValidationError::InvalidChannelConfig(_, _)
        ));
    }
}
