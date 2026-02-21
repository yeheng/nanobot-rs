//! Feishu (飞书) webhook handler

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
use crate::channels::feishu::{FeishuChannel, FeishuConfig, FeishuEvent, FeishuChallenge, FeishuChallengeResponse};

/// Feishu webhook handler that wraps a FeishuChannel
pub struct FeishuWebhookHandler {
    channel: Arc<tokio::sync::RwLock<FeishuChannel>>,
    path: String,
}

impl FeishuWebhookHandler {
    /// Create a new Feishu webhook handler
    pub fn new(channel: FeishuChannel, path: Option<&str>) -> Self {
        Self {
            channel: Arc::new(tokio::sync::RwLock::new(channel)),
            path: path.unwrap_or("/feishu/events").to_string(),
        }
    }

    /// Create from config and inbound processor
    pub fn from_config(
        config: FeishuConfig,
        inbound_processor: Arc<dyn InboundProcessor>,
        path: Option<&str>,
    ) -> Self {
        let channel = FeishuChannel::new(config, inbound_processor);
        Self::new(channel, path)
    }
}

#[async_trait]
impl WebhookHandler for FeishuWebhookHandler {
    fn path(&self) -> &str {
        &self.path
    }

    async fn handle_get(
        &self,
        _query: Query<serde_json::Value>,
    ) -> WebhookResult<Response<Body>> {
        // Feishu doesn't use GET for URL verification, it uses POST with challenge
        debug!("Feishu GET request (unexpected)");
        Ok(handlers::bad_request("Use POST for Feishu webhooks"))
    }

    async fn handle_post(
        &self,
        _headers: HeaderMap,
        _query: Query<serde_json::Value>,
        body: bytes::Bytes,
    ) -> WebhookResult<Response<Body>> {
        debug!("Feishu callback POST request");

        // Parse body as JSON value first to detect the type
        let json_value: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
            WebhookError::InvalidBody(format!("Invalid JSON body: {}", e))
        })?;

        // Check if this is a URL verification challenge
        if let Some(challenge_type) = json_value.get("type").and_then(|t| t.as_str()) {
            if challenge_type == "url_verification" {
                return self.handle_challenge(&json_value).await;
            }
        }

        // Regular event handling
        let event: FeishuEvent = serde_json::from_value(json_value).map_err(|e| {
            WebhookError::InvalidBody(format!("Invalid event format: {}", e))
        })?;

        let mut channel = self.channel.write().await;

        match channel.handle_webhook_event(event).await {
            Ok(()) => {
                debug!("Feishu event processed successfully");
                // Feishu expects a JSON response
                Ok(handlers::json_response(axum::http::StatusCode::OK, &serde_json::json!({"code": 0})))
            }
            Err(e) => {
                error!("Feishu event processing failed: {}", e);
                // Return error response
                Ok(handlers::json_response(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    &serde_json::json!({"code": -1, "msg": e.to_string()}),
                ))
            }
        }
    }
}

impl FeishuWebhookHandler {
    /// Handle URL verification challenge from Feishu
    async fn handle_challenge(&self, json_value: &serde_json::Value) -> WebhookResult<Response<Body>> {
        debug!("Feishu URL verification challenge");

        let challenge: FeishuChallenge = serde_json::from_value(json_value.clone()).map_err(|e| {
            WebhookError::InvalidBody(format!("Invalid challenge format: {}", e))
        })?;

        // Verify token if configured
        // Note: Token verification is done in handle_webhook_event
        // For challenge, we just need to respond with the challenge string

        let response = FeishuChallengeResponse {
            challenge: challenge.challenge,
        };

        info!("Feishu URL verification successful");
        Ok(handlers::json_response(axum::http::StatusCode::OK, &response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::middleware::NoopInboundProcessor;

    fn create_test_config() -> FeishuConfig {
        FeishuConfig {
            app_id: "cli_test123".to_string(),
            app_secret: "test_secret".to_string(),
            verification_token: Some("test_token".to_string()),
            encrypt_key: None,
            allow_from: vec![],
        }
    }

    #[test]
    fn test_feishu_webhook_handler_creation() {
        let config = create_test_config();
        let handler = FeishuWebhookHandler::from_config(
            config,
            Arc::new(NoopInboundProcessor),
            Some("/custom/feishu"),
        );
        assert_eq!(handler.path(), "/custom/feishu");
    }

    #[test]
    fn test_default_path() {
        let config = create_test_config();
        let handler = FeishuWebhookHandler::from_config(config, Arc::new(NoopInboundProcessor), None);
        assert_eq!(handler.path(), "/feishu/events");
    }

    #[test]
    fn test_challenge_response_serialization() {
        let response = FeishuChallengeResponse {
            challenge: "test_challenge_string".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("test_challenge_string"));
    }
}
