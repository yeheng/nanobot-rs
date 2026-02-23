//! DingTalk (钉钉) webhook handler

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    body::Body,
    extract::Query,
    http::{HeaderMap, Response},
};
use tokio::sync::mpsc::Sender;
use tracing::{debug, error};

use super::handlers;
use super::types::{WebhookError, WebhookHandler, WebhookResult};
use crate::bus::events::InboundMessage;
use crate::channels::dingtalk::{DingTalkCallbackMessage, DingTalkChannel, DingTalkConfig};

/// DingTalk webhook handler that wraps a DingTalkChannel
pub struct DingTalkWebhookHandler {
    channel: Arc<tokio::sync::RwLock<DingTalkChannel>>,
    path: String,
}

impl DingTalkWebhookHandler {
    /// Create a new DingTalk webhook handler
    pub fn new(channel: DingTalkChannel, path: Option<&str>) -> Self {
        Self {
            channel: Arc::new(tokio::sync::RwLock::new(channel)),
            path: path.unwrap_or("/dingtalk/callback").to_string(),
        }
    }

    /// Create from config and inbound sender
    pub fn from_config(
        config: DingTalkConfig,
        inbound_sender: Sender<InboundMessage>,
        path: Option<&str>,
    ) -> Self {
        let channel = DingTalkChannel::new(config, inbound_sender);
        Self::new(channel, path)
    }
}

#[async_trait]
impl WebhookHandler for DingTalkWebhookHandler {
    fn path(&self) -> &str {
        &self.path
    }

    async fn handle_get(&self, _query: Query<serde_json::Value>) -> WebhookResult<Response<Body>> {
        // DingTalk doesn't use GET for callbacks
        debug!("DingTalk GET request (unexpected)");
        Ok(handlers::bad_request("Use POST for DingTalk webhooks"))
    }

    async fn handle_post(
        &self,
        _headers: HeaderMap,
        _query: Query<serde_json::Value>,
        body: bytes::Bytes,
    ) -> WebhookResult<Response<Body>> {
        debug!("DingTalk callback POST request");

        // Parse the callback message
        let message: DingTalkCallbackMessage = serde_json::from_slice(&body)
            .map_err(|e| WebhookError::InvalidBody(format!("Invalid request body: {}", e)))?;

        let channel = self.channel.read().await;

        match channel.handle_callback_message(message).await {
            Ok(()) => {
                debug!("DingTalk callback processed successfully");
                // DingTalk expects a JSON response with success
                Ok(handlers::json_response(
                    axum::http::StatusCode::OK,
                    &serde_json::json!({"msg": "success"}),
                ))
            }
            Err(e) => {
                error!("DingTalk callback processing failed: {}", e);
                // Return success anyway to avoid retries for non-recoverable errors
                Ok(handlers::json_response(
                    axum::http::StatusCode::OK,
                    &serde_json::json!({"msg": "success"}),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn create_test_sender() -> Sender<InboundMessage> {
        let (tx, _rx) = mpsc::channel(100);
        tx
    }

    fn create_test_config() -> DingTalkConfig {
        DingTalkConfig {
            webhook_url: "https://oapi.dingtalk.com/robot/send?access_token=test123".to_string(),
            secret: Some("test_secret".to_string()),
            access_token: None,
            allow_from: vec![],
        }
    }

    #[test]
    fn test_dingtalk_webhook_handler_creation() {
        let config = create_test_config();
        let handler = DingTalkWebhookHandler::from_config(
            config,
            create_test_sender(),
            Some("/custom/dingtalk"),
        );
        assert_eq!(handler.path(), "/custom/dingtalk");
    }

    #[test]
    fn test_default_path() {
        let config = create_test_config();
        let handler = DingTalkWebhookHandler::from_config(config, create_test_sender(), None);
        assert_eq!(handler.path(), "/dingtalk/callback");
    }
}
