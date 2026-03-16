//! Feishu (飞书) webhook handler
//!
//! Provides Axum routes for handling Feishu callbacks.
//!
//! The webhook handler is **decoupled** from `FeishuChannel`: it only needs
//! the platform config (for token verification) and an `InboundSender`
//! (to forward parsed messages to the bus). No `Arc<RwLock<Channel>>` needed.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::IntoResponse,
    Router,
};
use tracing::{debug, error, info};

use super::handlers;
use crate::bus::events::InboundMessage;
use crate::bus::ChannelType;
use crate::channels::feishu::{
    FeishuChallenge, FeishuChallengeResponse, FeishuConfig, FeishuEvent, FeishuTextContent,
};
use crate::channels::middleware::InboundSender;

/// State for Feishu webhook routes.
///
/// Holds only the data needed for inbound processing — no channel lock.
#[derive(Clone)]
pub struct FeishuState {
    pub config: Arc<FeishuConfig>,
    pub inbound_sender: InboundSender,
}

impl FeishuState {
    /// Create from config and inbound sender
    pub fn from_config(config: FeishuConfig, inbound_sender: InboundSender) -> Self {
        Self {
            config: Arc::new(config),
            inbound_sender,
        }
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

    // Regular event handling — no lock needed, just config + sender
    let event: FeishuEvent = match serde_json::from_value(json_value) {
        Ok(e) => e,
        Err(e) => {
            return handlers::bad_request(&format!("Invalid event format: {}", e));
        }
    };

    match process_webhook_event(&state.config, &state.inbound_sender, event).await {
        Ok(()) => {
            debug!("Feishu event processed successfully");
            handlers::json_response(axum::http::StatusCode::OK, &serde_json::json!({"code": 0}))
        }
        Err(e) => {
            error!("Feishu event processing failed: {}", e);
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

    let response = FeishuChallengeResponse {
        challenge: challenge.challenge,
    };

    info!("Feishu URL verification successful");
    handlers::json_response(axum::http::StatusCode::OK, &response)
}

// ── Standalone webhook processing (no Channel dependency) ───

/// Process an incoming Feishu webhook event.
///
/// Only needs `FeishuConfig` (for token verification) and `InboundSender`
/// (to forward the parsed message). No `FeishuChannel` lock required.
async fn process_webhook_event(
    config: &FeishuConfig,
    inbound_sender: &InboundSender,
    event: FeishuEvent,
) -> anyhow::Result<()> {
    // Verify token if configured
    if let Some(ref token) = config.verification_token {
        if event.token != *token {
            error!("Invalid verification token in Feishu event");
            return Ok(());
        }
    }

    match event.event_type.as_str() {
        "im.message.receive_v1" => {
            if let Some(message) = event.event.message {
                process_message_event(config, inbound_sender, message, event.event.sender).await?;
            }
        }
        _ => {
            debug!("Ignoring Feishu event type: {}", event.event_type);
        }
    }

    Ok(())
}

/// Process a Feishu message receive event.
async fn process_message_event(
    config: &FeishuConfig,
    inbound_sender: &InboundSender,
    message: crate::channels::feishu::FeishuMessage,
    sender: Option<crate::channels::feishu::FeishuSender>,
) -> anyhow::Result<()> {
    if let Some(ref sender_info) = sender {
        let sender_id = &sender_info.sender_id.user_id;
        if !config.allow_from.is_empty() && !config.allow_from.contains(sender_id) {
            debug!(
                "Ignoring message from unauthorized Feishu user: {}",
                sender_id
            );
            return Ok(());
        }

        // Only handle text messages
        if message.message_type != "text" {
            debug!("Ignoring non-text Feishu message: {}", message.message_type);
            return Ok(());
        }

        // Parse message content (JSON string for text)
        let content = serde_json::from_str::<FeishuTextContent>(&message.content)
            .map(|c| c.text)
            .unwrap_or_else(|_| message.content.clone());

        debug!("Received Feishu message: {}", content);

        let metadata = serde_json::to_value(&message).ok();

        let inbound = InboundMessage {
            channel: ChannelType::Feishu,
            sender_id: sender_info.sender_id.user_id.clone(),
            chat_id: message.chat_id.clone(),
            content,
            media: None,
            metadata,
            timestamp: chrono::Utc::now(),
            trace_id: None,
        };

        inbound_sender.send(inbound).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn create_test_sender() -> InboundSender {
        let (tx, _rx) = mpsc::channel(100);
        InboundSender::new(tx)
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
        assert!(Arc::strong_count(&state.config) >= 1);
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
