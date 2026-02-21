//! WeCom (企业微信) webhook handler

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    body::Body,
    extract::Query,
    http::{HeaderMap, Response},
};
use tracing::{debug, error, info};

use super::handlers;
use super::types::{WebhookError, WebhookHandler, WebhookResult};
use crate::channels::middleware::InboundProcessor;
use crate::channels::wecom::{WeComCallbackBody, WeComCallbackQuery, WeComChannel, WeComConfig};

/// WeCom webhook handler that wraps a WeComChannel
pub struct WeComWebhookHandler {
    channel: Arc<tokio::sync::RwLock<WeComChannel>>,
    path: String,
}

impl WeComWebhookHandler {
    /// Create a new WeCom webhook handler
    pub fn new(channel: WeComChannel, path: Option<&str>) -> Self {
        Self {
            channel: Arc::new(tokio::sync::RwLock::new(channel)),
            path: path.unwrap_or("/wecom/callback").to_string(),
        }
    }

    /// Create from config and inbound processor
    pub fn from_config(
        config: WeComConfig,
        inbound_processor: Arc<dyn InboundProcessor>,
        path: Option<&str>,
    ) -> Self {
        let channel = WeComChannel::new(config, inbound_processor);
        Self::new(channel, path)
    }
}

#[async_trait]
impl WebhookHandler for WeComWebhookHandler {
    fn path(&self) -> &str {
        &self.path
    }

    async fn handle_get(
        &self,
        Query(query): Query<serde_json::Value>,
    ) -> WebhookResult<Response<Body>> {
        debug!("WeCom URL verification request: {:?}", query);

        // Parse query parameters
        let callback_query: WeComCallbackQuery =
            serde_json::from_value(query).map_err(|e| {
                WebhookError::InvalidBody(format!("Invalid query parameters: {}", e))
            })?;

        let channel = self.channel.read().await;

        match channel.verify_url(&callback_query) {
            Ok(echostr) => {
                info!("WeCom URL verification successful");
                Ok(handlers::success(&echostr))
            }
            Err(e) => {
                error!("WeCom URL verification failed: {}", e);
                Ok(handlers::bad_request(&format!("Verification failed: {}", e)))
            }
        }
    }

    async fn handle_post(
        &self,
        _headers: HeaderMap,
        Query(query): Query<serde_json::Value>,
        body: bytes::Bytes,
    ) -> WebhookResult<Response<Body>> {
        debug!("WeCom callback POST request");

        // Parse query parameters
        let callback_query: WeComCallbackQuery =
            serde_json::from_value(query).map_err(|e| {
                WebhookError::InvalidBody(format!("Invalid query parameters: {}", e))
            })?;

        // Parse body
        let callback_body: WeComCallbackBody =
            serde_json::from_slice(&body).map_err(|e| {
                WebhookError::InvalidBody(format!("Invalid request body: {}", e))
            })?;

        let channel = self.channel.read().await;

        match channel.handle_callback_message(&callback_query, &callback_body).await {
            Ok(()) => {
                debug!("WeCom callback processed successfully");
                // WeCom expects "success" as response
                Ok(handlers::success("success"))
            }
            Err(e) => {
                error!("WeCom callback processing failed: {}", e);
                // Still return success to avoid retries for non-recoverable errors
                Ok(handlers::success("success"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::middleware::NoopInboundProcessor;

    fn create_test_config() -> WeComConfig {
        WeComConfig {
            corpid: "ww_test123".to_string(),
            corpsecret: "test_secret".to_string(),
            agent_id: 1000001,
            token: Some("test_token".to_string()),
            encoding_aes_key: Some("MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY".to_string()),
            allow_from: vec![],
        }
    }

    #[test]
    fn test_wecom_webhook_handler_creation() {
        let config = create_test_config();
        let handler = WeComWebhookHandler::from_config(
            config,
            Arc::new(NoopInboundProcessor),
            Some("/custom/wecom"),
        );
        assert_eq!(handler.path(), "/custom/wecom");
    }

    #[test]
    fn test_default_path() {
        let config = create_test_config();
        let handler = WeComWebhookHandler::from_config(config, Arc::new(NoopInboundProcessor), None);
        assert_eq!(handler.path(), "/wecom/callback");
    }
}
