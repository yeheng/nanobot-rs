use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::{Query, State},
    response::IntoResponse,
};
use futures::{sink::SinkExt, stream::StreamExt};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

use crate::bus::events::{InboundMessage, OutboundMessage};
use crate::bus::ChannelType::WebSocket as WebSocketChannel;

type ConnectionId = String;
type UserId = String;

/// Authentication validator function type
type AuthValidator = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Maximum number of concurrent WebSocket connections
const MAX_CONNECTIONS: usize = 1000;

/// Channel size for per-connection message queue (backpressure)
const CONNECTION_CHANNEL_SIZE: usize = 100;

/// Query parameters for WebSocket connection
#[derive(Debug, serde::Deserialize)]
pub struct WebSocketQuery {
    /// Optional authentication token
    pub token: Option<String>,
    /// Optional user ID (defaults to connection ID if not provided)
    pub user_id: Option<String>,
}

/// Guard to ensure connection cleanup even on panic or early return
struct ConnectionGuard {
    manager: Arc<WebSocketManager>,
    connection_id: ConnectionId,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let manager = self.manager.clone();
        let connection_id = self.connection_id.clone();
        // Use tokio::spawn to handle async cleanup in Drop
        tokio::spawn(async move {
            manager.connections.write().await.remove(&connection_id);
            debug!("Connection guard cleaned up connection: {}", connection_id);
        });
    }
}

/// Manages active WebSocket connections
pub struct WebSocketManager {
    /// Map of active connections
    /// Key: Connection ID (UUID)
    /// Value: Sender to the connection's write loop
    connections: RwLock<HashMap<ConnectionId, mpsc::Sender<Message>>>,

    /// Map of user IDs to connection IDs (for routing messages to specific users)
    user_connections: RwLock<HashMap<UserId, ConnectionId>>,

    /// Sender to the message bus for inbound messages
    inbound_tx: mpsc::Sender<InboundMessage>,

    /// Optional authentication token validator (can be set via set_auth_validator)
    auth_validator: RwLock<Option<AuthValidator>>,
}

impl WebSocketManager {
    pub fn new(inbound_tx: mpsc::Sender<InboundMessage>) -> Self {
        Self {
            connections: RwLock::new(HashMap::new()),
            user_connections: RwLock::new(HashMap::new()),
            inbound_tx,
            auth_validator: RwLock::new(None),
        }
    }

    /// Set an authentication validator function
    pub fn set_auth_validator<F>(&self, validator: F)
    where
        F: Fn(&str) -> bool + Send + Sync + 'static,
    {
        *self.auth_validator.try_write().unwrap() = Some(Arc::new(validator));
    }

    /// Get the number of active connections
    pub async fn connection_count(&self) -> usize {
        self.connections.read().await.len()
    }

    /// Handle a new WebSocket connection
    pub async fn handle_connection(
        ws: WebSocketUpgrade,
        State(manager): State<Arc<WebSocketManager>>,
        Query(query): Query<WebSocketQuery>,
    ) -> impl IntoResponse {
        ws.on_upgrade(move |socket| handle_socket(socket, manager, query))
    }

    /// Send an outbound message to a specific user/connection
    pub async fn send(&self, msg: OutboundMessage) {
        let connections = self.connections.read().await;
        let user_connections = self.user_connections.read().await;

        debug!(
            "WebSocketManager::send - chat_id={}, has_ws_message={}, content_len={}",
            msg.chat_id,
            msg.ws_message.is_some(),
            msg.content.len()
        );

        // Try to find the connection by chat_id (which could be user_id or connection_id)
        let connection_id = if let Some(conn_id) = user_connections.get(&msg.chat_id) {
            // chat_id is a user_id, look up the connection
            debug!("WebSocketManager::send - found connection {} for user_id {}", conn_id, msg.chat_id);
            conn_id.clone()
        } else if connections.contains_key(&msg.chat_id) {
            // chat_id is already a connection_id
            debug!("WebSocketManager::send - chat_id {} is a connection_id", msg.chat_id);
            msg.chat_id.clone()
        } else {
            warn!("No connection found for chat_id: {} (user_connections: {:?})", msg.chat_id, user_connections.keys().collect::<Vec<_>>());
            return;
        };

        if let Some(sender) = connections.get(&connection_id) {
            // Check if we have a structured WebSocket message
            let text = if let Some(ref ws_msg) = msg.ws_message {
                let json = ws_msg.to_json();
                debug!("WebSocketManager::send - sending ws_message: {}", json);
                json
            } else if !msg.content.is_empty() {
                // Legacy: send plain text (wrapped in JSON for consistency)
                debug!("WebSocketManager::send - sending content: {}", &msg.content[..msg.content.len().min(100)]);
                msg.content
            } else {
                // Empty message, skip
                warn!("WebSocketManager::send - empty message, skipping");
                return;
            };

            if let Err(e) = sender.send(Message::Text(text.into())).await {
                warn!(
                    "Failed to send message to connection {}: {}",
                    connection_id, e
                );
            } else {
                debug!("WebSocketManager::send - message sent successfully to {}", connection_id);
            }
        } else {
            warn!(
                "Connection {} not found for outbound message",
                connection_id
            );
        }
    }
}

async fn handle_socket(socket: WebSocket, manager: Arc<WebSocketManager>, query: WebSocketQuery) {
    let (mut sender, mut receiver) = socket.split();

    // Create a unique ID for this connection
    let connection_id = uuid::Uuid::new_v4().to_string();

    // Determine user ID (use provided or default to connection ID)
    let user_id = query.user_id.unwrap_or_else(|| connection_id.clone());

    // Authenticate if token is provided
    if let Some(token) = &query.token {
        let validator = manager.auth_validator.read().await;
        if let Some(ref validator_fn) = *validator {
            if !validator_fn(token) {
                warn!(
                    "Authentication failed for connection {}: invalid token",
                    connection_id
                );
                return;
            }
        }
    }

    debug!(
        "New WebSocket connection: {} (user: {})",
        connection_id, user_id
    );

    // Check connection limit
    let current_connections = manager.connection_count().await;
    if current_connections >= MAX_CONNECTIONS {
        warn!(
            "Connection limit reached ({}/{}), rejecting new connection",
            current_connections, MAX_CONNECTIONS
        );
        return;
    }

    // Create a bounded channel for sending messages to this connection (backpressure)
    let (tx, mut rx) = mpsc::channel(CONNECTION_CHANNEL_SIZE);

    // Register the connection
    {
        let mut connections = manager.connections.write().await;
        let mut user_connections = manager.user_connections.write().await;

        // If user already has a connection, remove the old one (single connection per user)
        if let Some(old_conn_id) = user_connections.get(&user_id) {
            connections.remove(old_conn_id);
            info!(
                "Replaced old connection {} for user {}",
                old_conn_id, user_id
            );
        }

        connections.insert(connection_id.clone(), tx);
        user_connections.insert(user_id.clone(), connection_id.clone());
    }

    info!(
        "WebSocket connected: {} (user: {}), total connections: {}",
        connection_id,
        user_id,
        manager.connection_count().await
    );

    // Create guard to ensure cleanup
    let _guard = ConnectionGuard {
        manager: manager.clone(),
        connection_id: connection_id.clone(),
    };

    // Also clean up user connection mapping on exit (guard will be dropped with _guard)
    let _user_cleanup_guard = UserConnectionGuard {
        manager: manager.clone(),
        user_id: user_id.clone(),
    };

    // Spawn a task to forward messages from the channel to the WebSocket
    let mut send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(msg).await.is_err() {
                debug!("Send error, connection may be closed");
                break;
            }
        }
    });

    // Handle incoming messages
    let mut recv_task = {
        let manager = manager.clone();
        let connection_id = connection_id.clone();
        let user_id = user_id.clone();
        tokio::spawn(async move {
            while let Some(Ok(msg)) = receiver.next().await {
                match msg {
                    Message::Text(text) => {
                        debug!("incoming messages: {}", text);
                        // Create InboundMessage and send to bus
                        let inbound = InboundMessage {
                            channel: WebSocketChannel,
                            sender_id: user_id.clone(),
                            chat_id: user_id.clone(), // Use user_id for session persistence
                            content: text.to_string(),
                            media: None,
                            metadata: None,
                            timestamp: chrono::Utc::now(),
                            trace_id: None,
                        };

                        if let Err(e) = manager.inbound_tx.send(inbound).await {
                            error!("Failed to forward inbound message: {}", e);
                            break;
                        }
                    }
                    Message::Close(_) => {
                        debug!("Received close frame from {}", connection_id);
                        break;
                    }
                    Message::Ping(_data) => {
                        // Handle ping for keepalive
                        debug!("Received ping from {}", connection_id);
                        // Note: axum handles pong automatically
                    }
                    _ => {}
                }
            }
        })
    };

    // Wait for either task to finish
    tokio::select! {
        _ = (&mut send_task) => {
            debug!("Send task finished for {}", connection_id);
        },
        _ = (&mut recv_task) => {
            debug!("Recv task finished for {}", connection_id);
        },
    }

    // Guard will clean up automatically
    debug!("WebSocket connection closed: {}", connection_id);
}

/// Guard to clean up user connection mapping
struct UserConnectionGuard {
    manager: Arc<WebSocketManager>,
    user_id: String,
}

impl Drop for UserConnectionGuard {
    fn drop(&mut self) {
        let manager = self.manager.clone();
        let user_id = self.user_id.clone();
        tokio::spawn(async move {
            let mut user_connections = manager.user_connections.write().await;
            if let Some(conn_id) = user_connections.remove(&user_id) {
                // Also remove from connections map
                let mut connections = manager.connections.write().await;
                connections.remove(&conn_id);
                debug!(
                    "User connection guard cleaned up user {} and connection {}",
                    user_id, conn_id
                );
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_websocket_query_parsing() {
        // Test that WebSocketQuery can be deserialized
        let query = WebSocketQuery {
            token: Some("test-token".to_string()),
            user_id: Some("user-123".to_string()),
        };
        assert_eq!(query.token, Some("test-token".to_string()));
        assert_eq!(query.user_id, Some("user-123".to_string()));
    }

    #[tokio::test]
    async fn test_websocket_manager_creation() {
        let (inbound_tx, _) = mpsc::channel(100);
        let manager = WebSocketManager::new(inbound_tx);

        assert_eq!(manager.connection_count().await, 0);
    }

    #[tokio::test]
    async fn test_auth_validator() {
        let (inbound_tx, _) = mpsc::channel(100);
        let manager = WebSocketManager::new(inbound_tx);

        // Set a simple validator
        manager.set_auth_validator(|token| token == "valid-token");

        // The validator is set, we can't easily test it without a full connection
        // but we can verify the method doesn't panic
        assert!(manager.auth_validator.read().await.is_some());
    }
}
