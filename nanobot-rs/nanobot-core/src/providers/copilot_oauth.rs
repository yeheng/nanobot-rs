//! GitHub Copilot OAuth Device Flow implementation
//!
//! Implements the OAuth 2.0 Device Authorization Grant flow for GitHub Copilot.
//! This allows users to authenticate without a local web server.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, info};

/// Default GitHub App Client ID for Copilot
/// This is the official GitHub Copilot extension's client ID
pub const DEFAULT_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";

/// OAuth endpoints
const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";

/// Copilot token exchange endpoint
pub const COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";

/// Device code response from GitHub
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
    /// The device verification code
    pub device_code: String,
    /// The code the user should enter
    pub user_code: String,
    /// The URL for user to visit
    pub verification_uri: String,
    /// Seconds until the code expires
    pub expires_in: u32,
    /// Recommended polling interval in seconds
    pub interval: u32,
}

/// Access token response from GitHub
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessTokenResponse {
    /// The access token (only present on success)
    #[serde(default)]
    pub access_token: Option<String>,
    /// Token type (usually "bearer")
    #[serde(default)]
    pub token_type: Option<String>,
    /// Scope of the token
    #[serde(default)]
    pub scope: Option<String>,
    /// Error code (only present on failure)
    #[serde(default)]
    pub error: Option<String>,
    /// Error description
    #[serde(default)]
    pub error_description: Option<String>,
}

/// Copilot API endpoints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopilotEndpoints {
    /// API endpoint
    pub api: String,
    /// Origin tracker endpoint
    #[serde(rename = "origin-tracker")]
    pub origin_tracker: String,
    /// Proxy endpoint
    pub proxy: String,
    /// Telemetry endpoint
    pub telemetry: String,
}

/// Limited user quotas
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitedUserQuotas {
    /// Chat quota
    pub chat: u32,
    /// Completions quota
    pub completions: u32,
}

/// Copilot token response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopilotTokenResponse {
    /// Agent mode auto approval enabled
    #[serde(default)]
    pub agent_mode_auto_approval: bool,
    /// Annotations enabled
    #[serde(default)]
    pub annotations_enabled: bool,
    /// Azure only restriction
    #[serde(default)]
    pub azure_only: bool,
    /// Blackbird clientside indexing enabled
    #[serde(default)]
    pub blackbird_clientside_indexing: bool,
    /// Chat enabled
    #[serde(default)]
    pub chat_enabled: bool,
    /// Chat JetBrains enabled
    #[serde(default)]
    pub chat_jetbrains_enabled: bool,
    /// Code quote enabled
    #[serde(default)]
    pub code_quote_enabled: bool,
    /// Code review enabled
    #[serde(default)]
    pub code_review_enabled: bool,
    /// Code search enabled
    #[serde(default)]
    pub codesearch: bool,
    /// Copilot ignore enabled
    #[serde(default)]
    pub copilotignore_enabled: bool,
    /// API endpoints
    pub endpoints: CopilotEndpoints,
    /// Token expiration timestamp (Unix timestamp)
    pub expires_at: u64,
    /// Is individual account
    #[serde(default)]
    pub individual: bool,
    /// Limited user quotas (for free tier)
    #[serde(default)]
    pub limited_user_quotas: Option<LimitedUserQuotas>,
    /// Limited user reset date (Unix timestamp)
    #[serde(default)]
    pub limited_user_reset_date: Option<u64>,
    /// Prompt 8k enabled
    #[serde(default)]
    pub prompt_8k: bool,
    /// Public suggestions setting
    #[serde(default)]
    pub public_suggestions: String,
    /// Seconds until token refresh needed
    pub refresh_in: u32,
    /// SKU (e.g., "free_limited_copilot", "individual")
    #[serde(default)]
    pub sku: String,
    /// Snippy load test enabled
    #[serde(default)]
    pub snippy_load_test_enabled: bool,
    /// Telemetry setting
    #[serde(default)]
    pub telemetry: String,
    /// The Copilot token
    pub token: String,
    /// Tracking ID
    #[serde(default)]
    pub tracking_id: String,
    /// VS Code electron fetcher v2 enabled
    #[serde(default)]
    pub vsc_electron_fetcher_v2: bool,
    /// Xcode support enabled
    #[serde(default)]
    pub xcode: bool,
    /// Xcode chat enabled
    #[serde(default)]
    pub xcode_chat: bool,
}

/// OAuth errors
#[derive(Debug, Clone, thiserror::Error)]
pub enum OAuthError {
    #[error("Device code expired")]
    DeviceCodeExpired,

    #[error("Authorization pending - user has not completed the flow")]
    AuthorizationPending,

    #[error("Access denied by user")]
    AccessDenied,

    #[error("Invalid client ID")]
    InvalidClientId,

    #[error("Failed to obtain access token: {0}")]
    TokenError(String),

    #[error("HTTP request failed: {0}")]
    HttpError(String),

    #[error("JSON parsing error: {0}")]
    JsonError(String),
}

impl From<reqwest::Error> for OAuthError {
    fn from(e: reqwest::Error) -> Self {
        OAuthError::HttpError(e.to_string())
    }
}

/// Copilot OAuth client
pub struct CopilotOAuth {
    client: Client,
    client_id: String,
}

impl CopilotOAuth {
    /// Create a new OAuth client with the given client ID
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            client_id: client_id.into(),
        }
    }

    /// Create a new OAuth client with the default client ID
    pub fn with_default_client_id() -> Self {
        Self::new(DEFAULT_CLIENT_ID)
    }

    /// Step 1: Request a device code from GitHub
    ///
    /// Returns a device code and user code for the user to enter on GitHub
    pub async fn request_device_code(&self) -> Result<DeviceCodeResponse, OAuthError> {
        let payload = serde_json::json!({
            "client_id": self.client_id,
            "scope": "read:user"
        });

        debug!(
            "Requesting device code from GitHub with client_id: {}",
            self.client_id
        );
        debug!("Request URL: {}", DEVICE_CODE_URL);
        debug!(
            "Request payload: {}",
            serde_json::to_string(&payload).unwrap_or_default()
        );

        let request = self
            .client
            .post(DEVICE_CODE_URL)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("User-Agent", "GitHubCopilotChat/0.26.7")
            .json(&payload)
            .build()?;

        debug!("Request headers: {:?}", request.headers());

        let response = self.client.execute(request).await?;

        let status = response.status();
        let text = response.text().await?;

        debug!("Device code response status: {}, body: {}", status, text);

        if !status.is_success() {
            return Err(OAuthError::TokenError(format!(
                "GitHub returned error {}: {}",
                status, text
            )));
        }

        // Check if response looks like JSON
        let trimmed = text.trim();
        if !trimmed.starts_with('{') {
            return Err(OAuthError::JsonError(format!(
                "Expected JSON response, got: {}",
                trimmed
            )));
        }

        let device_code: DeviceCodeResponse =
            serde_json::from_str(&text).map_err(|e| OAuthError::JsonError(e.to_string()))?;

        Ok(device_code)
    }

    /// Step 2: Poll for access token
    ///
    /// This should be called repeatedly until the user completes authorization
    /// or the device code expires
    pub async fn poll_for_token(
        &self,
        device_code: &str,
    ) -> Result<AccessTokenResponse, OAuthError> {
        let payload = serde_json::json!({
            "client_id": self.client_id.as_str(),
            "device_code": device_code,
            "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
        });

        debug!("Polling for access token. URL: {}", ACCESS_TOKEN_URL);
        debug!(
            "Request payload: {}",
            serde_json::to_string(&payload).unwrap_or_default()
        );

        let request = self
            .client
            .post(ACCESS_TOKEN_URL)
            .header("Accept", "application/json")
            .header("User-Agent", "GitHubCopilotChat/0.26.7")
            .json(&payload)
            .build()?;

        debug!("Request headers: {:?}", request.headers());

        let response = self.client.execute(request).await?;

        let status = response.status();
        let text = response.text().await?;
        debug!("Access token response status: {}, body: {}", status, text);

        let token_response: AccessTokenResponse =
            serde_json::from_str(&text).map_err(|e| OAuthError::JsonError(e.to_string()))?;

        // Check for errors
        if let Some(error) = &token_response.error {
            return Err(match error.as_str() {
                "authorization_pending" => OAuthError::AuthorizationPending,
                "slow_down" => OAuthError::AuthorizationPending,
                "expired_token" => OAuthError::DeviceCodeExpired,
                "access_denied" => OAuthError::AccessDenied,
                "incorrect_client_credentials" => OAuthError::InvalidClientId,
                _ => OAuthError::TokenError(error.clone()),
            });
        }

        Ok(token_response)
    }

    /// Run the complete OAuth Device Flow
    ///
    /// This will:
    /// 1. Request a device code
    /// 2. Display instructions to the user
    /// 3. Poll until the user completes authorization
    /// 4. Return the access token
    pub async fn start_device_flow(&self) -> Result<String, OAuthError> {
        // Step 1: Get device code
        let device_code = self.request_device_code().await?;

        // Step 2: Display instructions
        println!("\n{}", "To authenticate with GitHub Copilot:".bold());
        println!();
        println!("  1. Open: {}", device_code.verification_uri.cyan());
        println!("  2. Enter code: {}", device_code.user_code.bold().yellow());
        println!();
        println!("  (Code expires in {} seconds)", device_code.expires_in);
        println!();

        // Step 3: Poll for completion
        let interval = Duration::from_secs(device_code.interval as u64);
        let mut elapsed = 0u32;
        let expires_in = device_code.expires_in;

        loop {
            tokio::time::sleep(interval).await;
            elapsed += device_code.interval;

            match self.poll_for_token(&device_code.device_code).await {
                Ok(response) => {
                    if let Some(token) = response.access_token {
                        info!("Successfully obtained GitHub access token");
                        return Ok(token);
                    }
                }
                Err(OAuthError::AuthorizationPending) => {
                    // Continue polling
                    debug!("Authorization pending... ({}/{})", elapsed, expires_in);
                    print!(".");
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                }
                Err(e) => return Err(e),
            }

            // Check if expired
            if elapsed >= expires_in {
                return Err(OAuthError::DeviceCodeExpired);
            }
        }
    }

    /// Exchange a GitHub access token for a Copilot JWT token
    ///
    /// The Copilot token is short-lived (~30 minutes) and must be refreshed
    pub async fn get_copilot_token(
        &self,
        github_token: &str,
    ) -> Result<CopilotTokenResponse, OAuthError> {
        debug!("Getting copilot token. URL: {}", COPILOT_TOKEN_URL);

        let request = self
            .client
            .get(COPILOT_TOKEN_URL)
            .header("Authorization", format!("Bearer {}", github_token))
            .header("User-Agent", "GitHubCopilotChat/0.26.7")
            .header("Editor-Version", "vscode/1.99.3")
            .header("Editor-Plugin-Version", "copilot-chat/0.26.7")
            .header("Accept", "application/json")
            .header("Copilot-Integration-Id", "vscode-chat")
            .build()?;

        debug!("Request headers: {:?}", request.headers());

        let response = self.client.execute(request).await?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        debug!("------> Response status: {}", status);
        debug!("------> Response body: {}", body);
        if !status.is_success() {
            return Err(OAuthError::TokenError(format!(
                "Failed to get Copilot token: {} - {}",
                status, body
            )));
        }

        let token_response: CopilotTokenResponse = serde_json::from_str(&body)
            .map_err(|e| OAuthError::JsonError(format!("{}: {}", e, body)))?;

        Ok(token_response)
    }

    /// Validate a Personal Access Token
    ///
    /// Returns true if the token is valid and has Copilot access
    pub async fn validate_pat(&self, pat: &str) -> Result<bool, OAuthError> {
        match self.get_copilot_token(pat).await {
            Ok(_) => Ok(true),
            Err(OAuthError::TokenError(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

// Helper trait for colored output (only when terminal is available)
trait Bold {
    fn bold(&self) -> String;
}

impl Bold for str {
    fn bold(&self) -> String {
        format!("\x1b[1m{}\x1b[0m", self)
    }
}

trait Cyan {
    fn cyan(&self) -> String;
}

impl Cyan for str {
    fn cyan(&self) -> String {
        format!("\x1b[36m{}\x1b[0m", self)
    }
}

trait Yellow {
    fn yellow(&self) -> String;
}

impl Yellow for str {
    fn yellow(&self) -> String {
        format!("\x1b[33m{}\x1b[0m", self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::const_is_empty)]
    fn test_default_client_id() {
        assert!(
            !DEFAULT_CLIENT_ID.is_empty(),
            "DEFAULT_CLIENT_ID should not be empty"
        );
    }

    #[test]
    fn test_oauth_client_creation() {
        let oauth = CopilotOAuth::new("test_client_id");
        assert_eq!(oauth.client_id, "test_client_id");
    }

    #[test]
    fn test_oauth_client_default() {
        let oauth = CopilotOAuth::with_default_client_id();
        assert_eq!(oauth.client_id, DEFAULT_CLIENT_ID);
    }

    #[test]
    fn test_device_code_response_parsing() {
        let json = r#"{
            "device_code": "abc123",
            "user_code": "XXXX-XXXX",
            "verification_uri": "https://github.com/login/device",
            "expires_in": 900,
            "interval": 5
        }"#;

        let response: DeviceCodeResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.device_code, "abc123");
        assert_eq!(response.user_code, "XXXX-XXXX");
        assert_eq!(response.expires_in, 900);
    }

    #[test]
    fn test_access_token_response_parsing() {
        let json = r#"{
            "access_token": "gho_xxx",
            "token_type": "bearer",
            "scope": "user"
        }"#;

        let response: AccessTokenResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.access_token, Some("gho_xxx".to_string()));
        assert!(response.error.is_none());
    }

    #[test]
    fn test_access_token_error_parsing() {
        let json = r#"{
            "error": "authorization_pending",
            "error_description": "User has not completed the flow"
        }"#;

        let response: AccessTokenResponse = serde_json::from_str(json).unwrap();
        assert!(response.access_token.is_none());
        assert_eq!(response.error, Some("authorization_pending".to_string()));
    }

    #[test]
    fn test_copilot_token_response_parsing() {
        let json = r#"{
            "agent_mode_auto_approval": true,
            "annotations_enabled": true,
            "azure_only": false,
            "blackbird_clientside_indexing": false,
            "chat_enabled": true,
            "chat_jetbrains_enabled": true,
            "code_quote_enabled": true,
            "code_review_enabled": false,
            "codesearch": true,
            "copilotignore_enabled": false,
            "endpoints": {
                "api": "https://api.individual.githubcopilot.com",
                "origin-tracker": "https://origin-tracker.individual.githubcopilot.com",
                "proxy": "https://proxy.individual.githubcopilot.com",
                "telemetry": "https://telemetry.individual.githubcopilot.com"
            },
            "expires_at": 1772293626,
            "individual": true,
            "limited_user_quotas": {
                "chat": 490,
                "completions": 4000
            },
            "limited_user_reset_date": 1773187200,
            "prompt_8k": true,
            "public_suggestions": "disabled",
            "refresh_in": 1500,
            "sku": "free_limited_copilot",
            "snippy_load_test_enabled": false,
            "telemetry": "enabled",
            "token": "test_token_value",
            "tracking_id": "76fcb479514ca6833209aad18684554c",
            "vsc_electron_fetcher_v2": false,
            "xcode": true,
            "xcode_chat": false
        }"#;

        let response: CopilotTokenResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.token, "test_token_value");
        assert_eq!(response.expires_at, 1772293626);
        assert_eq!(response.refresh_in, 1500);
        assert_eq!(
            response.endpoints.api,
            "https://api.individual.githubcopilot.com"
        );
        assert_eq!(
            response.endpoints.proxy,
            "https://proxy.individual.githubcopilot.com"
        );
        assert!(response.agent_mode_auto_approval);
        assert!(response.chat_enabled);
        assert_eq!(response.sku, "free_limited_copilot");
        assert!(response.limited_user_quotas.is_some());
        let quotas = response.limited_user_quotas.unwrap();
        assert_eq!(quotas.chat, 490);
        assert_eq!(quotas.completions, 4000);
    }
}
