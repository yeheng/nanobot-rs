//! WeCom (企业微信) channel implementation
//!
//! Supports WeCom bot messaging via the Application Message API.
//! Uses corpid + corpsecret to obtain an access_token, then sends
//! messages to users/departments through agentid.

use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use super::base::Channel;
use super::middleware::InboundProcessor;
use crate::bus::events::OutboundMessage;
use crate::trail::TrailContext;

/// WeCom bot channel configuration
#[derive(Debug, Clone)]
pub struct WeComConfig {
    /// Corp ID
    pub corpid: String,

    /// Corp Secret
    pub corpsecret: String,

    /// Agent ID for the bot application
    pub agent_id: i64,

    /// Token for callback verification (optional)
    pub token: Option<String>,

    /// EncodingAESKey for callback message encryption/decryption (optional, 43 chars)
    pub encoding_aes_key: Option<String>,

    /// Allowed users (empty = allow all)
    pub allow_from: Vec<String>,
}

/// WeCom API response envelope
#[derive(Debug, Deserialize)]
struct WeComApiResponse {
    errcode: i32,
    errmsg: String,
}

/// WeCom bot channel with middleware support.
///
/// Uses `InboundProcessor` to process incoming messages through
/// the middleware stack before publishing to the bus.
pub struct WeComChannel {
    config: WeComConfig,
    #[allow(dead_code)]
    inbound_processor: Arc<dyn InboundProcessor>,
    #[allow(dead_code)]
    trail_ctx: TrailContext,
    client: Client,
    access_token: Option<String>,
}

impl WeComChannel {
    /// Create a new WeCom bot channel with an inbound processor.
    pub fn new(config: WeComConfig, inbound_processor: Arc<dyn InboundProcessor>) -> Self {
        Self {
            config,
            inbound_processor,
            trail_ctx: TrailContext::default(),
            client: Client::new(),
            access_token: None,
        }
    }

    /// Set the trail context for this channel.
    pub fn with_trail_context(mut self, ctx: TrailContext) -> Self {
        self.trail_ctx = ctx;
        self
    }

    /// Get access_token via corpid + corpsecret.
    ///
    /// Caches the token in `self.access_token`. Called automatically during `start()`.
    pub async fn get_access_token(&mut self) -> anyhow::Result<&str> {
        if let Some(ref token) = self.access_token {
            return Ok(token);
        }

        let url = format!(
            "https://qyapi.weixin.qq.com/cgi-bin/gettoken?corpid={}&corpsecret={}",
            self.config.corpid, self.config.corpsecret
        );

        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await?;
            anyhow::bail!("Failed to get WeCom access token: {} - {}", status, body);
        }

        #[derive(Deserialize)]
        struct TokenResponse {
            errcode: i32,
            errmsg: String,
            access_token: Option<String>,
            #[allow(dead_code)]
            expires_in: Option<i64>,
        }

        let token_resp: TokenResponse = response.json().await?;
        if token_resp.errcode != 0 {
            anyhow::bail!(
                "WeCom gettoken error (errcode={}): {}",
                token_resp.errcode,
                token_resp.errmsg
            );
        }

        let token = token_resp.access_token.ok_or_else(|| {
            anyhow::anyhow!("WeCom gettoken returned errcode=0 but no access_token")
        })?;

        self.access_token = Some(token);
        info!("Obtained WeCom access token");

        Ok(self
            .access_token
            .as_ref()
            .expect("access_token was just set"))
    }

    /// Send a POST to the message/send API and check the response.
    async fn post_message<T: Serialize>(&self, body: &T) -> anyhow::Result<()> {
        let token = self
            .access_token
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No access token. Call get_access_token first."))?;

        let url = format!(
            "https://qyapi.weixin.qq.com/cgi-bin/message/send?access_token={}",
            token
        );

        let response = self.client.post(&url).json(body).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await?;
            anyhow::bail!("Failed to send WeCom message: {} - {}", status, body);
        }

        let result: WeComApiResponse = response.json().await?;
        if result.errcode != 0 {
            anyhow::bail!(
                "WeCom message/send error (errcode={}): {}",
                result.errcode,
                result.errmsg
            );
        }
        Ok(())
    }

    /// Send a text message to users.
    ///
    /// `to_user` — pipe-separated user IDs, e.g. `"UserID1|UserID2"` or `"@all"`.
    pub async fn send_text(&self, to_user: &str, text: &str) -> anyhow::Result<()> {
        #[derive(Serialize)]
        struct Msg {
            touser: String,
            msgtype: String,
            agentid: i64,
            text: Content,
        }
        #[derive(Serialize)]
        struct Content {
            content: String,
        }

        self.post_message(&Msg {
            touser: to_user.to_string(),
            msgtype: "text".to_string(),
            agentid: self.config.agent_id,
            text: Content {
                content: text.to_string(),
            },
        })
        .await?;

        debug!("Sent WeCom text message to {}", to_user);
        Ok(())
    }

    /// Send a markdown message to users.
    ///
    /// `to_user` — pipe-separated user IDs, e.g. `"UserID1|UserID2"` or `"@all"`.
    pub async fn send_markdown(&self, to_user: &str, content: &str) -> anyhow::Result<()> {
        #[derive(Serialize)]
        struct Msg {
            touser: String,
            msgtype: String,
            agentid: i64,
            markdown: Content,
        }
        #[derive(Serialize)]
        struct Content {
            content: String,
        }

        self.post_message(&Msg {
            touser: to_user.to_string(),
            msgtype: "markdown".to_string(),
            agentid: self.config.agent_id,
            markdown: Content {
                content: content.to_string(),
            },
        })
        .await?;

        debug!("Sent WeCom markdown message to {}", to_user);
        Ok(())
    }
}

#[async_trait]
impl Channel for WeComChannel {
    fn name(&self) -> &str {
        "wecom"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        info!("Starting WeCom channel");
        self.get_access_token().await?;
        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        info!("Stopping WeCom channel");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> anyhow::Result<()> {
        self.send_text(&msg.chat_id, &msg.content).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::middleware::NoopInboundProcessor;

    #[test]
    fn test_wecom_config_creation() {
        let config = WeComConfig {
            corpid: "ww1234567890".to_string(),
            corpsecret: "secret".to_string(),
            agent_id: 1000002,
            token: Some("token123".to_string()),
            encoding_aes_key: Some("abcdefghijklmnopqrstuvwxyz01234567890ABCDEF".to_string()),
            allow_from: vec![],
        };

        assert_eq!(config.corpid, "ww1234567890");
        assert_eq!(config.agent_id, 1000002);
        assert_eq!(config.token.as_deref(), Some("token123"));
        assert_eq!(
            config.encoding_aes_key.as_deref(),
            Some("abcdefghijklmnopqrstuvwxyz01234567890ABCDEF")
        );
    }

    #[test]
    fn test_wecom_channel_creation() {
        let config = WeComConfig {
            corpid: "ww_test".to_string(),
            corpsecret: "secret".to_string(),
            agent_id: 1000002,
            token: None,
            encoding_aes_key: None,
            allow_from: vec![],
        };

        let channel = WeComChannel::new(config, Arc::new(NoopInboundProcessor));
        assert_eq!(channel.name(), "wecom");
    }

    #[test]
    fn test_wecom_text_message_serialization() {
        #[derive(Serialize)]
        struct Msg {
            touser: String,
            msgtype: String,
            agentid: i64,
            text: Content,
        }
        #[derive(Serialize)]
        struct Content {
            content: String,
        }

        let message = Msg {
            touser: "UserID1|UserID2".to_string(),
            msgtype: "text".to_string(),
            agentid: 1000002,
            text: Content {
                content: "Hello".to_string(),
            },
        };

        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains("\"touser\":\"UserID1|UserID2\""));
        assert!(json.contains("\"agentid\":1000002"));
        assert!(json.contains("\"msgtype\":\"text\""));
        assert!(json.contains("\"content\":\"Hello\""));
    }

    #[test]
    fn test_wecom_markdown_message_serialization() {
        #[derive(Serialize)]
        struct Msg {
            touser: String,
            msgtype: String,
            agentid: i64,
            markdown: Content,
        }
        #[derive(Serialize)]
        struct Content {
            content: String,
        }

        let message = Msg {
            touser: "@all".to_string(),
            msgtype: "markdown".to_string(),
            agentid: 1000002,
            markdown: Content {
                content: "# Title\nBody".to_string(),
            },
        };

        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains("\"touser\":\"@all\""));
        assert!(json.contains("\"msgtype\":\"markdown\""));
        assert!(json.contains("\"agentid\":1000002"));
        assert!(json.contains("# Title\\nBody"));
    }
}
