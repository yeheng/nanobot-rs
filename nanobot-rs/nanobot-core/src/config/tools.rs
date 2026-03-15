//! Tools configuration schemas
//!
//! Configuration for various tools (Web, MCP, Exec, etc.)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Tools configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolsConfig {
    /// Restrict file operations to workspace
    #[serde(default, alias = "restrictToWorkspace")]
    pub restrict_to_workspace: bool,

    /// Web tools configuration
    #[serde(default)]
    pub web: WebToolsConfig,

    /// MCP servers (new grouped format)
    #[serde(default)]
    pub mcp: McpServersConfig,

    /// MCP servers (legacy flat format, for backward compatibility)
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,

    /// Exec tool configuration
    #[serde(default)]
    pub exec: ExecToolConfig,

    /// Maximum number of results returned by history_search (default: 15)
    #[serde(default = "default_history_search_limit", alias = "historySearchLimit")]
    pub history_search_limit: usize,
}

// ── Web Tools ─────────────────────────────────────────────────────────────

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

// ── MCP Server ────────────────────────────────────────────────────────────

/// MCP servers configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpServersConfig {
    /// Stdio-based MCP servers
    #[serde(default)]
    pub stdio: HashMap<String, StdioMcpConfig>,

    /// Remote HTTP-based MCP servers
    #[serde(default)]
    pub remote: HashMap<String, RemoteMcpConfig>,
}

/// Stdio MCP server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StdioMcpConfig {
    /// Command to run
    pub command: String,

    /// Arguments for the command
    #[serde(default)]
    pub args: Vec<String>,

    /// Environment variables
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
}

// ── MCP Remote Server Configuration (Enhanced) ─────────────────────────────

/// Authentication configuration for remote MCP servers
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct McpAuthConfig {
    /// API key sent as X-API-Key header
    #[serde(default, alias = "apiKey")]
    pub api_key: Option<String>,

    /// Bearer token sent as Authorization: Bearer header
    #[serde(default, alias = "bearerToken")]
    pub bearer_token: Option<String>,

    /// Custom headers to include in requests
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
}

/// Transport type configuration for remote MCP servers
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum RemoteTransportConfig {
    /// HTTP transport (POST JSON-RPC)
    Http { url: String },
    /// Server-Sent Events transport
    Sse { url: String },
    /// WebSocket transport
    #[serde(rename = "websocket")]
    WebSocket { url: String },
}

/// Health check configuration for remote MCP servers
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpHealthConfig {
    /// Enable health checking
    #[serde(default = "default_health_enabled")]
    pub enabled: bool,

    /// Health check interval in seconds
    #[serde(default = "default_health_interval")]
    pub interval: u64,

    /// Number of consecutive failures before marking unhealthy
    #[serde(default = "default_health_failure_threshold")]
    pub failure_threshold: u32,
}

impl Default for McpHealthConfig {
    fn default() -> Self {
        Self {
            enabled: default_health_enabled(),
            interval: default_health_interval(),
            failure_threshold: default_health_failure_threshold(),
        }
    }
}

fn default_health_enabled() -> bool {
    false
}

fn default_health_interval() -> u64 {
    60
}

fn default_health_failure_threshold() -> u32 {
    3
}

/// Retry configuration for remote MCP servers
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpRetryConfig {
    /// Enable automatic retries
    #[serde(default = "default_retry_enabled")]
    pub enabled: bool,

    /// Maximum number of retry attempts
    #[serde(default = "default_retry_max_attempts")]
    pub max_attempts: u32,

    /// Initial backoff delay in milliseconds
    #[serde(default = "default_retry_initial_backoff")]
    pub initial_backoff_ms: u64,

    /// Maximum backoff delay in milliseconds
    #[serde(default = "default_retry_max_backoff")]
    pub max_backoff_ms: u64,
}

impl Default for McpRetryConfig {
    fn default() -> Self {
        Self {
            enabled: default_retry_enabled(),
            max_attempts: default_retry_max_attempts(),
            initial_backoff_ms: default_retry_initial_backoff(),
            max_backoff_ms: default_retry_max_backoff(),
        }
    }
}

fn default_retry_enabled() -> bool {
    false
}

fn default_retry_max_attempts() -> u32 {
    3
}

fn default_retry_initial_backoff() -> u64 {
    1000
}

fn default_retry_max_backoff() -> u64 {
    30000
}

/// Default MCP request timeout in seconds
fn default_mcp_timeout() -> u64 {
    30
}

/// Remote MCP server configuration with enhanced features
///
/// Supports both simple format (backward compatible) and enhanced format:
///
/// ```yaml
/// # Simple format (backward compatible)
/// simple-server:
///   url: https://api.example.com/mcp
///
/// # Enhanced format
/// enhanced-server:
///   type: http
///   url: https://api.example.com/mcp
///   auth:
///     api_key: "${MCP_API_KEY}"
///     headers:
///       X-Custom: value
///   timeout: 60
///   health:
///     enabled: true
///     interval: 30
///   retry:
///     enabled: true
///     max_attempts: 3
/// ```
#[derive(Debug, Clone)]
pub enum RemoteMcpConfig {
    /// Simple format: just a URL (backward compatible)
    Simple { url: String },
    /// Enhanced format with all options
    Enhanced {
        transport: RemoteTransportConfig,
        auth: McpAuthConfig,
        health: McpHealthConfig,
        retry: McpRetryConfig,
        timeout: u64,
    },
}

impl<'de> Deserialize<'de> for RemoteMcpConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        let value = serde_json::Value::deserialize(deserializer)?;

        // Check if it's a simple format (only has "url" field, no "type")
        if let Some(obj) = value.as_object() {
            if let Some(url) = obj.get("url").and_then(|v| v.as_str()) {
                // If there's no "type" field, it's the simple format
                if !obj.contains_key("type") {
                    return Ok(RemoteMcpConfig::Simple {
                        url: url.to_string(),
                    });
                }
            }
        }

        // Otherwise, parse as enhanced format
        #[derive(Deserialize)]
        struct EnhancedConfig {
            #[serde(flatten)]
            transport: RemoteTransportConfig,
            #[serde(default)]
            auth: McpAuthConfig,
            #[serde(default)]
            health: McpHealthConfig,
            #[serde(default)]
            retry: McpRetryConfig,
            #[serde(default = "default_mcp_timeout")]
            timeout: u64,
        }

        let enhanced: EnhancedConfig =
            serde_json::from_value(value).map_err(|e| D::Error::custom(e.to_string()))?;

        Ok(RemoteMcpConfig::Enhanced {
            transport: enhanced.transport,
            auth: enhanced.auth,
            health: enhanced.health,
            retry: enhanced.retry,
            timeout: enhanced.timeout,
        })
    }
}

impl Serialize for RemoteMcpConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            RemoteMcpConfig::Simple { url } => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("url", url)?;
                map.end()
            }
            RemoteMcpConfig::Enhanced {
                transport,
                auth,
                health,
                retry,
                timeout,
            } => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(None)?;
                // Serialize transport (which includes type and url)
                match transport {
                    RemoteTransportConfig::Http { url } => {
                        map.serialize_entry("type", "http")?;
                        map.serialize_entry("url", url)?;
                    }
                    RemoteTransportConfig::Sse { url } => {
                        map.serialize_entry("type", "sse")?;
                        map.serialize_entry("url", url)?;
                    }
                    RemoteTransportConfig::WebSocket { url } => {
                        map.serialize_entry("type", "websocket")?;
                        map.serialize_entry("url", url)?;
                    }
                }
                if auth != &McpAuthConfig::default() {
                    map.serialize_entry("auth", auth)?;
                }
                if health != &McpHealthConfig::default() {
                    map.serialize_entry("health", health)?;
                }
                if retry != &McpRetryConfig::default() {
                    map.serialize_entry("retry", retry)?;
                }
                if *timeout != default_mcp_timeout() {
                    map.serialize_entry("timeout", timeout)?;
                }
                map.end()
            }
        }
    }
}

impl RemoteMcpConfig {
    /// Get the URL for this configuration
    pub fn url(&self) -> &str {
        match self {
            RemoteMcpConfig::Simple { url } => url,
            RemoteMcpConfig::Enhanced { transport, .. } => match transport {
                RemoteTransportConfig::Http { url } => url,
                RemoteTransportConfig::Sse { url } => url,
                RemoteTransportConfig::WebSocket { url } => url,
            },
        }
    }

    /// Get the transport type as a string
    pub fn transport_type(&self) -> &'static str {
        match self {
            RemoteMcpConfig::Simple { .. } => "http",
            RemoteMcpConfig::Enhanced { transport, .. } => match transport {
                RemoteTransportConfig::Http { .. } => "http",
                RemoteTransportConfig::Sse { .. } => "sse",
                RemoteTransportConfig::WebSocket { .. } => "websocket",
            },
        }
    }

    /// Get authentication configuration
    pub fn auth(&self) -> &McpAuthConfig {
        match self {
            RemoteMcpConfig::Simple { .. } => &MCP_AUTH_DEFAULT,
            RemoteMcpConfig::Enhanced { auth, .. } => auth,
        }
    }

    /// Get health check configuration
    pub fn health(&self) -> &McpHealthConfig {
        match self {
            RemoteMcpConfig::Simple { .. } => &MCP_HEALTH_DEFAULT,
            RemoteMcpConfig::Enhanced { health, .. } => health,
        }
    }

    /// Get retry configuration
    pub fn retry(&self) -> &McpRetryConfig {
        match self {
            RemoteMcpConfig::Simple { .. } => &MCP_RETRY_DEFAULT,
            RemoteMcpConfig::Enhanced { retry, .. } => retry,
        }
    }

    /// Get timeout in seconds
    pub fn timeout(&self) -> u64 {
        match self {
            RemoteMcpConfig::Simple { .. } => default_mcp_timeout(),
            RemoteMcpConfig::Enhanced { timeout, .. } => *timeout,
        }
    }
}

/// Default auth config for simple format
static MCP_AUTH_DEFAULT: McpAuthConfig = McpAuthConfig {
    api_key: None,
    bearer_token: None,
    headers: None,
};

/// Default health config for simple format
static MCP_HEALTH_DEFAULT: McpHealthConfig = McpHealthConfig {
    enabled: false,
    interval: 60,
    failure_threshold: 3,
};

/// Default retry config for simple format
static MCP_RETRY_DEFAULT: McpRetryConfig = McpRetryConfig {
    enabled: false,
    max_attempts: 3,
    initial_backoff_ms: 1000,
    max_backoff_ms: 30000,
};

/// MCP server configuration (legacy, for backward compatibility)
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

// ── Exec Tool ─────────────────────────────────────────────────────────────

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

// ── Default Functions ─────────────────────────────────────────────────────

fn default_true() -> bool {
    true
}

fn default_sandbox_backend() -> String {
    "bwrap".to_string()
}

fn default_tmp_size_mb() -> u32 {
    64
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

fn default_history_search_limit() -> usize {
    15
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exec_config_backward_compatible() {
        // Old config with only timeout should still parse
        let yaml = r#"
timeout: 60
"#;
        let exec: ExecToolConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(exec.timeout, 60);
        assert!(!exec.sandbox.enabled);
        assert!(exec.policy.allowlist.is_empty());
        assert_eq!(exec.limits.max_memory_mb, 512);
    }

    #[test]
    fn test_exec_config_with_sandbox() {
        let yaml = r#"
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
        let exec: ExecToolConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(exec.sandbox.enabled);
        assert_eq!(exec.sandbox.backend, "bwrap");
        assert_eq!(exec.sandbox.tmp_size_mb, 128);
        assert_eq!(exec.workspace, Some("/home/user/workspace".to_string()));
        assert_eq!(exec.policy.allowlist, vec!["ls", "cat", "git"]);
        assert_eq!(exec.policy.denylist, vec!["rm -rf /", "mkfs"]);
        assert_eq!(exec.limits.max_memory_mb, 1024);
        assert_eq!(exec.limits.max_cpu_secs, 30);
        assert_eq!(exec.limits.max_output_bytes, 2097152);
    }

    #[test]
    fn test_web_tools_config_with_proxy() {
        let yaml = r#"
searchProvider: brave
braveApiKey: test-brave-key
httpProxy: http://127.0.0.1:7890
httpsProxy: http://127.0.0.1:7890
socks5Proxy: socks5://127.0.0.1:1080
useEnvProxy: false
"#;
        let web: WebToolsConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(web.search_provider, Some("brave".to_string()));
        assert_eq!(web.brave_api_key, Some("test-brave-key".to_string()));
        assert_eq!(web.http_proxy, Some("http://127.0.0.1:7890".to_string()));
        assert_eq!(web.https_proxy, Some("http://127.0.0.1:7890".to_string()));
        assert_eq!(
            web.socks5_proxy,
            Some("socks5://127.0.0.1:1080".to_string())
        );
        assert!(!web.use_env_proxy);
    }

    #[test]
    fn test_web_tools_config_defaults() {
        let yaml = r#"
braveApiKey: test-key
"#;
        let web: WebToolsConfig = serde_yaml::from_str(yaml).unwrap();
        // use_env_proxy defaults to true
        assert!(web.use_env_proxy);
        // Proxy fields should be None when not specified
        assert!(web.http_proxy.is_none());
        assert!(web.https_proxy.is_none());
        assert!(web.socks5_proxy.is_none());
    }

    #[test]
    fn test_tools_config_full() {
        let yaml = r#"
restrictToWorkspace: true
web:
  searchProvider: brave
  braveApiKey: test-key
mcp_servers:
  filesystem:
    command: npx
    args:
      - "-y"
      - "@modelcontextprotocol/server-filesystem"
exec:
  timeout: 60
"#;
        let tools: ToolsConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(tools.restrict_to_workspace);
        assert_eq!(tools.web.search_provider, Some("brave".to_string()));
        assert_eq!(tools.exec.timeout, 60);
        assert!(tools.mcp_servers.contains_key("filesystem"));
    }

    // ── MCP Remote Configuration Tests ─────────────────────────────────────

    #[test]
    fn test_remote_mcp_config_simple() {
        let yaml = r#"
url: https://api.example.com/mcp
"#;
        let config: RemoteMcpConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.url(), "https://api.example.com/mcp");
        assert_eq!(config.transport_type(), "http");
        assert_eq!(config.timeout(), 30);
        assert!(!config.health().enabled);
        assert!(!config.retry().enabled);
    }

    #[test]
    fn test_remote_mcp_config_enhanced_http() {
        let yaml = r#"
type: http
url: https://api.example.com/mcp
auth:
  apiKey: test-key
  headers:
    X-Custom: value
timeout: 60
health:
  enabled: true
  interval: 30
retry:
  enabled: true
  max_attempts: 5
"#;
        let config: RemoteMcpConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.url(), "https://api.example.com/mcp");
        assert_eq!(config.transport_type(), "http");
        assert_eq!(config.timeout(), 60);
        assert!(config.health().enabled);
        assert_eq!(config.health().interval, 30);
        assert!(config.retry().enabled);
        assert_eq!(config.retry().max_attempts, 5);
        assert_eq!(config.auth().api_key, Some("test-key".to_string()));
        assert_eq!(
            config.auth().headers.as_ref().unwrap().get("X-Custom"),
            Some(&"value".to_string())
        );
    }

    #[test]
    fn test_remote_mcp_config_sse() {
        let yaml = r#"
type: sse
url: https://events.example.com/mcp
auth:
  bearerToken: test-token
"#;
        let config: RemoteMcpConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.url(), "https://events.example.com/mcp");
        assert_eq!(config.transport_type(), "sse");
        assert_eq!(config.auth().bearer_token, Some("test-token".to_string()));
    }

    #[test]
    fn test_remote_mcp_config_websocket() {
        let yaml = r#"
type: websocket
url: wss://ws.example.com/mcp
timeout: 45
"#;
        let config: RemoteMcpConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.url(), "wss://ws.example.com/mcp");
        assert_eq!(config.transport_type(), "websocket");
        assert_eq!(config.timeout(), 45);
    }

    #[test]
    fn test_mcp_servers_config_mixed() {
        let yaml = r#"
stdio:
  filesystem:
    command: npx
    args:
      - "-y"
      - "@modelcontextprotocol/server-filesystem"
remote:
  simple-server:
    url: https://api.example.com/mcp
  enhanced-server:
    type: http
    url: https://enhanced.example.com/mcp
    auth:
      apiKey: secret-key
    timeout: 60
"#;
        let config: McpServersConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.stdio.contains_key("filesystem"));
        assert!(config.remote.contains_key("simple-server"));
        assert!(config.remote.contains_key("enhanced-server"));

        let simple = config.remote.get("simple-server").unwrap();
        assert_eq!(simple.transport_type(), "http");

        let enhanced = config.remote.get("enhanced-server").unwrap();
        assert_eq!(enhanced.timeout(), 60);
        assert_eq!(enhanced.auth().api_key, Some("secret-key".to_string()));
    }
}
