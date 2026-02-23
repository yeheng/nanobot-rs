//! Feishu (飞书) webhook handler
//!
//! Provides Axum routes for handling Feishu callbacks.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::IntoResponse,
    Router,
};
use tokio::sync::{mpsc::Sender, RwLock};
use tracing::{debug, error, info};

use super::handlers;
use crate::bus::events::InboundMessage;
use crate::channels::feishu::{
    FeishuChallenge, FeishuChallengeResponse, FeishuChannel, FeishuConfig, FeishuEvent,
};

/// State for Feishu webhook routes
#[derive(Clone)]
pub struct FeishuState {
    pub channel: Arc<RwLock<FeishuChannel>>,
}

impl FeishuState {
    /// Create new Feishu state from a channel
    pub fn new(channel: FeishuChannel) -> Self {
        Self {
            channel: Arc::new(RwLock::new(channel)),
        }
    }

    /// Create from config and inbound sender
    pub fn from_config(config: FeishuConfig, inbound_sender: Sender<InboundMessage>) -> Self {
        let channel = FeishuChannel::new(config, inbound_sender);
        Self::new(channel)
    }
}

/// Create a router for Feishu webhook endpoints
pub fn create_feishu_routes(state: FeishuState, path: Option<&str>) -> Router {
    let path = path.unwrap_or("/feishu/events");
    Router::new()
        .route(path, axum::routing::get(handle_get).post(handle_post))
        .with_state(state)
}

/// Handle GET request (unexpected for Feishu)
async fn handle_get(
    _state: State<FeishuState>,
    _query: Query<serde_json::Value>,
) -> impl IntoResponse {
    // Feishu doesn't use GET for URL verification, it uses POST with challenge
    debug!("Feishu GET request (unexpected)");
    handlers::bad_request("Use POST for Feishu webhooks")
}

/// Handle POST request (message callback or challenge)
async fn handle_post(
    State(state): State<FeishuState>,
    _headers: HeaderMap,
    _query: Query<serde_json::Value>,
    body: bytes::Bytes,
) -> impl IntoResponse {
    debug!("Feishu callback POST request");

    // Parse body as JSON value first to detect the type
    let json_value: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return handlers::bad_request(&format!("Invalid JSON body: {}", e));
        }
    };

    // Check if this is a URL verification challenge
    if let Some(challenge_type) = json_value.get("type").and_then(|t| t.as_str()) {
        if challenge_type == "url_verification" {
            return handle_challenge(&json_value).into_response();
        }
    }

    // Regular event handling
    let event: FeishuEvent = match serde_json::from_value(json_value) {
        Ok(e) => e,
        Err(e) => {
            return handlers::bad_request(&format!("Invalid event format: {}", e));
        }
    };

    let mut channel = state.channel.write().await;

    match channel.handle_webhook_event(event).await {
        Ok(()) => {
            debug!("Feishu event processed successfully");
            // Feishu expects a JSON response
            handlers::json_response(axum::http::StatusCode::OK, &serde_json::json!({"code": 0}))
        }
        Err(e) => {
            error!("Feishu event processing failed: {}", e);
            // Return error response
            handlers::json_response(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                &serde_json::json!({"code": -1, "msg": e.to_string()}),
            )
        }
    }
}

/// Handle URL verification challenge from Feishu
fn handle_challenge(json_value: &serde_json::Value) -> impl IntoResponse {
    debug!("Feishu URL verification challenge");

    let challenge: FeishuChallenge = match serde_json::from_value(json_value.clone()) {
        Ok(c) => c,
        Err(e) => {
            return handlers::bad_request(&format!("Invalid challenge format: {}", e));
        }
    };

    // Verify token if configured
    // Note: Token verification is done in handle_webhook_event
    // For challenge, we just need to respond with the challenge string

    let response = FeishuChallengeResponse {
        challenge: challenge.challenge,
    };

    info!("Feishu URL verification successful");
    handlers::json_response(axum::http::StatusCode::OK, &response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn create_test_sender() -> Sender<InboundMessage> {
        let (tx, _rx) = mpsc::channel(100);
        tx
    }

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
    fn test_feishu_state_creation() {
        let config = create_test_config();
        let state = FeishuState::from_config(config, create_test_sender());
        assert!(Arc::strong_count(&state.channel) >= 1);
    }

    #[test]
    fn test_create_feishu_routes_default_path() {
        let config = create_test_config();
        let state = FeishuState::from_config(config, create_test_sender());
        let _router = create_feishu_routes(state, None);
    }

    #[test]
    fn test_create_feishu_routes_custom_path() {
        let config = create_test_config();
        let state = FeishuState::from_config(config, create_test_sender());
        let _router = create_feishu_routes(state, Some("/custom/feishu"));
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
