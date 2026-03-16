//! WeCom (企业微信) webhook handler
//!
//! Provides Axum routes for handling WeCom callbacks.
//!
//! The webhook handler is **decoupled** from `WeComChannel`: it only needs
//! the platform config (for signature verification and decryption),
//! a pre-decoded AES key, and an `InboundSender`.
//! No `Arc<RwLock<Channel>>` needed.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::IntoResponse,
    routing::get,
    Router,
};
use tracing::{debug, error, info, warn};

use super::handlers;
use crate::bus::events::InboundMessage;
use crate::bus::ChannelType;
use crate::channels::middleware::InboundSender;
use crate::channels::wecom::{WeComCallbackBody, WeComCallbackQuery, WeComConfig};
use crate::crypto::wecom::{compute_signature, decode_aes_key, decrypt_message};

/// State for WeCom webhook routes.
///
/// Holds only the data needed for inbound processing — no channel lock.
#[derive(Clone)]
pub struct WeComState {
    pub config: Arc<WeComConfig>,
    /// Pre-decoded AES key (32 bytes) for message decryption.
    pub aes_key: Option<Vec<u8>>,
    pub inbound_sender: InboundSender,
}

impl WeComState {
    /// Create from config and inbound sender.
    ///
    /// Automatically decodes the `encoding_aes_key` if present.
    pub fn from_config(config: WeComConfig, inbound_sender: InboundSender) -> Self {
        let aes_key =
            config
                .encoding_aes_key
                .as_deref()
                .and_then(|key| match decode_aes_key(key) {
                    Ok(k) => Some(k),
                    Err(e) => {
                        warn!("Failed to decode WeCom encoding_aes_key: {}", e);
                        None
                    }
                });
        Self {
            config: Arc::new(config),
            aes_key,
            inbound_sender,
        }
    }
}

/// Create a router for WeCom webhook endpoints
pub fn create_wecom_routes(state: WeComState, path: Option<&str>) -> Router {
    let path = path.unwrap_or("/wecom/callback");
    Router::new()
        .route(path, get(handle_get).post(handle_post))
        .with_state(state)
}

/// Handle GET request (URL verification)
async fn handle_get(
    State(state): State<WeComState>,
    Query(query): Query<serde_json::Value>,
) -> impl IntoResponse {
    debug!("WeCom URL verification request: {:?}", query);

    // Parse query parameters
    let callback_query: WeComCallbackQuery = match serde_json::from_value(query) {
        Ok(q) => q,
        Err(e) => {
            return handlers::bad_request(&format!("Invalid query parameters: {}", e));
        }
    };

    match verify_url(&state.config, state.aes_key.as_deref(), &callback_query) {
        Ok(echostr) => {
            info!("WeCom URL verification successful");
            handlers::success(&echostr)
        }
        Err(e) => {
            error!("WeCom URL verification failed: {}", e);
            handlers::bad_request(&format!("Verification failed: {}", e))
        }
    }
}

/// Handle POST request (message callback)
async fn handle_post(
    State(state): State<WeComState>,
    _headers: HeaderMap,
    Query(query): Query<serde_json::Value>,
    body: bytes::Bytes,
) -> impl IntoResponse {
    debug!("WeCom callback POST request");

    // Parse query parameters
    let callback_query: WeComCallbackQuery = match serde_json::from_value(query) {
        Ok(q) => q,
        Err(e) => {
            return handlers::bad_request(&format!("Invalid query parameters: {}", e));
        }
    };

    // Parse body
    let callback_body: WeComCallbackBody = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => {
            return handlers::bad_request(&format!("Invalid request body: {}", e));
        }
    };

    match process_callback_message(
        &state.config,
        state.aes_key.as_deref(),
        &state.inbound_sender,
        &callback_query,
        &callback_body,
    )
    .await
    {
        Ok(()) => {
            debug!("WeCom callback processed successfully");
            handlers::success("success")
        }
        Err(e) => {
            error!("WeCom callback processing failed: {}", e);
            // Still return success to avoid retries for non-recoverable errors
            handlers::success("success")
        }
    }
}

// ── Standalone webhook processing (no Channel dependency) ───

/// Verify callback URL (handles the GET verification request from WeCom).
///
/// Only needs config fields and the pre-decoded AES key.
fn verify_url(
    config: &WeComConfig,
    aes_key: Option<&[u8]>,
    query: &WeComCallbackQuery,
) -> anyhow::Result<String> {
    let token = config
        .token
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Token not configured for callback verification"))?;

    let echostr = query
        .echostr
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Missing echostr in URL verification request"))?;

    // Verify signature
    let expected_sig = compute_signature(token, &query.timestamp, &query.nonce, echostr);
    if expected_sig != query.msg_signature {
        anyhow::bail!(
            "Signature mismatch in URL verification: expected={}, got={}",
            expected_sig,
            query.msg_signature
        );
    }

    // Decrypt echostr
    let key =
        aes_key.ok_or_else(|| anyhow::anyhow!("No AES key. encoding_aes_key not configured."))?;
    let plaintext = decrypt_message(key, echostr, &config.corpid)?;

    debug!("WeCom URL verification succeeded");
    Ok(plaintext)
}

/// Process an incoming WeCom callback message.
///
/// Verifies signature, decrypts, parses, and forwards via `InboundSender`.
async fn process_callback_message(
    config: &WeComConfig,
    aes_key: Option<&[u8]>,
    inbound_sender: &InboundSender,
    query: &WeComCallbackQuery,
    body: &WeComCallbackBody,
) -> anyhow::Result<()> {
    let token = config
        .token
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Token not configured for callback"))?;

    // Verify signature
    let expected_sig = compute_signature(token, &query.timestamp, &query.nonce, &body.encrypt);
    if expected_sig != query.msg_signature {
        error!(
            "WeCom callback signature mismatch: expected={}, got={}",
            expected_sig, query.msg_signature
        );
        anyhow::bail!("Signature verification failed for WeCom callback");
    }

    // Decrypt
    let key =
        aes_key.ok_or_else(|| anyhow::anyhow!("No AES key. encoding_aes_key not configured."))?;
    let xml_str = decrypt_message(key, &body.encrypt, &config.corpid)?;
    debug!("Decrypted WeCom callback message: {}", xml_str);

    // Parse the XML message — reuse the parser from the channel module
    let message = crate::channels::wecom::parse_callback_xml(&xml_str)?;

    // Check allowlist
    if !config.allow_from.is_empty() && !config.allow_from.contains(&message.from_user_name) {
        debug!(
            "Ignoring message from unauthorized WeCom user: {}",
            message.from_user_name
        );
        return Ok(());
    }

    // Handle by message type
    match message.msg_type.as_str() {
        "text" => {
            let content = message.content.as_deref().unwrap_or("");
            if content.is_empty() {
                debug!("Ignoring empty WeCom text message");
                return Ok(());
            }

            debug!(
                "Received WeCom text message from {}: {}",
                message.from_user_name, content
            );

            let inbound = InboundMessage {
                channel: ChannelType::Wecom,
                sender_id: message.from_user_name.clone(),
                chat_id: message.from_user_name.clone(),
                content: content.to_string(),
                media: None,
                metadata: serde_json::to_value(&message).ok(),
                timestamp: chrono::Utc::now(),
                trace_id: None,
            };

            inbound_sender.send(inbound).await?;
        }
        "event" => {
            debug!(
                "Received WeCom event: {:?} from {}",
                message.event, message.from_user_name
            );
        }
        other => {
            warn!(
                "Ignoring unsupported WeCom message type: {} from {}",
                other, message.from_user_name
            );
        }
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
    fn test_wecom_state_creation() {
        let config = create_test_config();
        let state = WeComState::from_config(config, create_test_sender());
        assert!(Arc::strong_count(&state.config) >= 1);
    }

    #[test]
    fn test_create_wecom_routes_default_path() {
        let config = create_test_config();
        let state = WeComState::from_config(config, create_test_sender());
        let _router = create_wecom_routes(state, None);
    }

    #[test]
    fn test_create_wecom_routes_custom_path() {
        let config = create_test_config();
        let state = WeComState::from_config(config, create_test_sender());
        let _router = create_wecom_routes(state, Some("/custom/wecom"));
    }
}
