//! Configuration schema
//!
//! Compatible with Python nanobot's config format (now uses YAML)

use crate::error::ConfigValidationError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Root configuration structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// LLM providers configuration
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,

    /// Agent configuration
    #[serde(default)]
    pub agents: AgentsConfig,

    /// Channel configurations
    #[serde(default)]
    pub channels: ChannelsConfig,

    /// Tools configuration
    #[serde(default)]
    pub tools: ToolsConfig,
}

/// Provider configuration (OpenAI, OpenRouter, Anthropic, etc.)
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfig {
    /// API key for the provider
    #[serde(default, alias = "apiKey")]
    pub api_key: Option<String>,

    /// API base URL (for custom endpoints)
    #[serde(default, alias = "apiBase")]
    pub api_base: Option<String>,

    /// Whether this provider supports thinking/reasoning mode
    /// (e.g., zhipu/glm-5, deepseek/deepseek-reasoner)
    /// If not set, defaults to known providers list
    #[serde(default, alias = "supportsThinking")]
    pub supports_thinking: Option<bool>,

    /// OAuth client ID for providers that support OAuth (e.g., GitHub Copilot)
    #[serde(default, alias = "clientId")]
    pub client_id: Option<String>,
}

impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("api_key", &self.api_key.as_ref().map(|_| "***REDACTED***"))
            .field("api_base", &self.api_base)
            .field("supports_thinking", &self.supports_thinking)
            .field(
                "client_id",
                &self.client_id.as_ref().map(|_| "***REDACTED***"),
            )
            .finish()
    }
}

impl ProviderConfig {
    /// Check if this provider supports thinking mode.
    ///
    /// Returns the explicit `supports_thinking` config value, defaulting to `false`.
    /// Providers must declare this capability in config rather than relying on a hardcoded list.
    pub fn supports_thinking(&self) -> bool {
        self.supports_thinking.unwrap_or(false)
    }

    /// Check if this provider is available (configured and has required credentials).
    ///
    /// Local providers (ollama, litellm) don't require an API key.
    /// Remote providers require a non-empty API key to be configured.
    pub fn is_available(&self, provider_name: &str) -> bool {
        let is_local = matches!(provider_name, "ollama" | "litellm");
        if is_local {
            return true;
        }
        // Check for non-empty API key
        self.api_key
            .as_ref()
            .is_some_and(|key| !key.trim().is_empty())
    }
}

/// Agents configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentsConfig {
    /// Default agent settings
    #[serde(default)]
    pub defaults: AgentDefaults,
}

/// Default agent settings
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentDefaults {
    /// Model to use
    #[serde(default)]
    pub model: Option<String>,

    /// Temperature for generation
    #[serde(default = "default_temperature")]
    pub temperature: f32,

    /// Maximum tokens to generate
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,

    /// Maximum tool call iterations
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,

    /// Memory window size
    #[serde(default = "default_memory_window")]
    pub memory_window: usize,

    /// Enable thinking/reasoning mode for deep reasoning models (GLM-5, DeepSeek R1, etc.)
    #[serde(default)]
    pub thinking_enabled: bool,

    /// Enable streaming mode for progressive output (default: true)
    #[serde(default = "default_streaming")]
    pub streaming: bool,
}

fn default_temperature() -> f32 {
    0.7
}
fn default_max_tokens() -> u32 {
    4096
}
fn default_max_iterations() -> u32 {
    20
}
fn default_memory_window() -> usize {
    50
}
fn default_streaming() -> bool {
    true
}

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

fn default_imap_port() -> u16 {
    993
}

fn default_smtp_port() -> u16 {
    587
}

fn default_true() -> bool {
    true
}

// ── Configuration Validation Methods ──────────────────────────────────

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

impl Config {
    /// Validate the entire configuration
    pub fn validate(&self) -> Result<(), Vec<ConfigValidationError>> {
        let mut errors = Vec::new();

        // Validate providers
        for (name, provider) in &self.providers {
            if !provider.is_available(name) {
                errors.push(ConfigValidationError::ProviderNotAvailable(name.clone()));
            }
        }

        // Validate channels
        errors.extend(self.channels.validate());

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Tools configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolsConfig {
    /// Restrict file operations to workspace
    #[serde(default, alias = "restrictToWorkspace")]
    pub restrict_to_workspace: bool,

    /// Web tools configuration
    #[serde(default)]
    pub web: WebToolsConfig,

    /// MCP servers
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,

    /// Exec tool configuration
    #[serde(default)]
    pub exec: ExecToolConfig,
}

/// Web tools configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebToolsConfig {
    /// Provider to use for web search (brave, tavily, exa, firecrawl)
    #[serde(default, alias = "searchProvider")]
    pub search_provider: Option<String>,

    /// Brave Search API key for web_search tool
    #[serde(default, alias = "braveApiKey")]
    pub brave_api_key: Option<String>,

    /// Tavily Search API key
    #[serde(default, alias = "tavilyApiKey")]
    pub tavily_api_key: Option<String>,

    /// Exa Search API key
    #[serde(default, alias = "exaApiKey")]
    pub exa_api_key: Option<String>,

    /// Firecrawl API key
    #[serde(default, alias = "firecrawlApiKey")]
    pub firecrawl_api_key: Option<String>,

    /// HTTP proxy URL (e.g., "http://127.0.0.1:7890")
    #[serde(default, alias = "httpProxy")]
    pub http_proxy: Option<String>,

    /// HTTPS proxy URL (e.g., "http://127.0.0.1:7890")
    #[serde(default, alias = "httpsProxy")]
    pub https_proxy: Option<String>,

    /// SOCKS5 proxy URL (e.g., "socks5://127.0.0.1:1080")
    #[serde(default, alias = "socks5Proxy")]
    pub socks5_proxy: Option<String>,

    /// Whether to use system proxy settings from environment variables
    /// (HTTP_PROXY, HTTPS_PROXY, ALL_PROXY). Default: true
    #[serde(default = "default_true", alias = "useEnvProxy")]
    pub use_env_proxy: bool,
}

/// MCP server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Command to run (for stdio transport)
    #[serde(default)]
    pub command: Option<String>,

    /// Arguments for the command
    #[serde(default)]
    pub args: Option<Vec<String>>,

    /// URL for HTTP transport
    #[serde(default)]
    pub url: Option<String>,
}

/// Exec tool configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecToolConfig {
    /// Default timeout in seconds
    #[serde(default = "default_exec_timeout")]
    pub timeout: u64,

    /// Workspace directory for agent file operations (default: $HOME/.nanobot)
    #[serde(default)]
    pub workspace: Option<String>,

    /// Sandbox configuration
    #[serde(default)]
    pub sandbox: SandboxConfig,

    /// Command policy (allowlist/denylist)
    #[serde(default)]
    pub policy: CommandPolicyConfig,

    /// Resource limits
    #[serde(default)]
    pub limits: ResourceLimitsConfig,
}

/// Sandbox execution backend configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Enable sandbox (default: false, opt-in)
    #[serde(default)]
    pub enabled: bool,

    /// Sandbox backend (currently only "bwrap" supported)
    #[serde(default = "default_sandbox_backend")]
    pub backend: String,

    /// Size of /tmp tmpfs inside sandbox in MB (default: 64)
    #[serde(default = "default_tmp_size_mb")]
    pub tmp_size_mb: u32,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: default_sandbox_backend(),
            tmp_size_mb: default_tmp_size_mb(),
        }
    }
}

fn default_sandbox_backend() -> String {
    "bwrap".to_string()
}
fn default_tmp_size_mb() -> u32 {
    64
}

/// Command allowlist/denylist policy configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CommandPolicyConfig {
    /// Allowed command binaries (first token). Empty = allow all.
    #[serde(default)]
    pub allowlist: Vec<String>,

    /// Denied command patterns (substring match). Empty = deny none.
    #[serde(default)]
    pub denylist: Vec<String>,
}

/// Resource limits for shell execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimitsConfig {
    /// Maximum memory in MB (default: 512)
    #[serde(default = "default_max_memory_mb")]
    pub max_memory_mb: u32,

    /// Maximum CPU time in seconds (default: 60)
    #[serde(default = "default_max_cpu_secs")]
    pub max_cpu_secs: u32,

    /// Maximum output size in bytes (default: 1MB)
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: usize,
}

impl Default for ResourceLimitsConfig {
    fn default() -> Self {
        Self {
            max_memory_mb: default_max_memory_mb(),
            max_cpu_secs: default_max_cpu_secs(),
            max_output_bytes: default_max_output_bytes(),
        }
    }
}

fn default_max_memory_mb() -> u32 {
    512
}
fn default_max_cpu_secs() -> u32 {
    60
}
fn default_max_output_bytes() -> usize {
    1_048_576 // 1 MB
}

fn default_exec_timeout() -> u64 {
    120
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_config() {
        let yaml = "";
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.providers.is_empty());
    }

    #[test]
    fn test_parse_provider_config() {
        let yaml = r#"
providers:
  openrouter:
    api_key: sk-or-v1-xxx
agents:
  defaults:
    model: anthropic/claude-opus-4-5
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.providers.get("openrouter").unwrap().api_key,
            Some("sk-or-v1-xxx".to_string())
        );
        assert_eq!(
            config.agents.defaults.model,
            Some("anthropic/claude-opus-4-5".to_string())
        );
    }

    #[test]
    fn test_exec_config_backward_compatible() {
        // Old config with only timeout should still parse
        let yaml = r#"
tools:
  exec:
    timeout: 60
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.tools.exec.timeout, 60);
        assert!(!config.tools.exec.sandbox.enabled);
        assert!(config.tools.exec.policy.allowlist.is_empty());
        assert_eq!(config.tools.exec.limits.max_memory_mb, 512);
    }

    #[test]
    fn test_exec_config_with_sandbox() {
        let yaml = r#"
tools:
  exec:
    timeout: 120
    workspace: /home/user/workspace
    sandbox:
      enabled: true
      backend: bwrap
      tmp_size_mb: 128
    policy:
      allowlist:
        - ls
        - cat
        - git
      denylist:
        - "rm -rf /"
        - mkfs
    limits:
      max_memory_mb: 1024
      max_cpu_secs: 30
      max_output_bytes: 2097152
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.tools.exec.sandbox.enabled);
        assert_eq!(config.tools.exec.sandbox.backend, "bwrap");
        assert_eq!(config.tools.exec.sandbox.tmp_size_mb, 128);
        assert_eq!(
            config.tools.exec.workspace,
            Some("/home/user/workspace".to_string())
        );
        assert_eq!(config.tools.exec.policy.allowlist, vec!["ls", "cat", "git"]);
        assert_eq!(config.tools.exec.policy.denylist, vec!["rm -rf /", "mkfs"]);
        assert_eq!(config.tools.exec.limits.max_memory_mb, 1024);
        assert_eq!(config.tools.exec.limits.max_cpu_secs, 30);
        assert_eq!(config.tools.exec.limits.max_output_bytes, 2097152);
    }

    #[test]
    fn test_web_tools_config_with_proxy() {
        let yaml = r#"
tools:
  web:
    searchProvider: brave
    braveApiKey: test-brave-key
    httpProxy: http://127.0.0.1:7890
    httpsProxy: http://127.0.0.1:7890
    socks5Proxy: socks5://127.0.0.1:1080
    useEnvProxy: false
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.tools.web.search_provider, Some("brave".to_string()));
        assert_eq!(
            config.tools.web.brave_api_key,
            Some("test-brave-key".to_string())
        );
        assert_eq!(
            config.tools.web.http_proxy,
            Some("http://127.0.0.1:7890".to_string())
        );
        assert_eq!(
            config.tools.web.https_proxy,
            Some("http://127.0.0.1:7890".to_string())
        );
        assert_eq!(
            config.tools.web.socks5_proxy,
            Some("socks5://127.0.0.1:1080".to_string())
        );
        assert!(!config.tools.web.use_env_proxy);
    }

    #[test]
    fn test_web_tools_config_defaults() {
        let yaml = r#"
tools:
  web:
    braveApiKey: test-key
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        // use_env_proxy defaults to true
        assert!(config.tools.web.use_env_proxy);
        // Proxy fields should be None when not specified
        assert!(config.tools.web.http_proxy.is_none());
        assert!(config.tools.web.https_proxy.is_none());
        assert!(config.tools.web.socks5_proxy.is_none());
    }

    // ── Configuration Validation Tests ──────────────────────────────────

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
    fn test_config_validate_provider() {
        // Provider without API key should fail
        let yaml = r#"
providers:
  openai:
    api_key: ""
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            ConfigValidationError::ProviderNotAvailable(_)
        ));

        // Local provider (ollama) doesn't need API key
        let yaml = r#"
providers:
  ollama:
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_config_validate_email() {
        let yaml = r#"
channels:
  email:
    enabled: true
    imapHost: imap.example.com
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(matches!(
            errors[0],
            ConfigValidationError::IncompleteEmailConfig
        ));
    }

    #[test]
    fn test_dingtalk_config_parsing() {
        let yaml = r#"
channels:
  dingtalk:
    enabled: true
    webhookUrl: https://oapi.dingtalk.com/robot/send?access_token=xxx
    secret: SECxxx
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.channels.dingtalk.is_some());
        let dingtalk = config.channels.dingtalk.unwrap();
        assert!(dingtalk.enabled);
        assert!(!dingtalk.webhook_url.is_empty());
        assert!(dingtalk.secret.is_some());
    }

    #[test]
    fn test_channels_config_enabled_count() {
        let yaml = r#"
channels:
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
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        // Telegram (enabled) + Email (enabled) = 2
        assert_eq!(config.channels.enabled_count(), 2);
    }
}
