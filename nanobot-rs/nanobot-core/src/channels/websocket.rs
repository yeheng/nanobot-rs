use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    response::IntoResponse,
};
use futures::{sink::SinkExt, stream::StreamExt};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, warn};

use crate::bus::events::{InboundMessage, OutboundMessage};
use crate::bus::ChannelType::WebSocket as WebSocketChannel;

type ConnectionId = String;

/// Manages active WebSocket connections
pub struct WebSocketManager {
    /// Map of active connections
    /// Key: Connection ID (UUID)
    /// Value: Sender to the connection's write loop
    connections: RwLock<HashMap<ConnectionId, mpsc::UnboundedSender<Message>>>,

    /// Sender to the message bus for inbound messages
    inbound_tx: mpsc::Sender<InboundMessage>,
}

impl WebSocketManager {
    pub fn new(inbound_tx: mpsc::Sender<InboundMessage>) -> Self {
        Self {
            connections: RwLock::new(HashMap::new()),
            inbound_tx,
        }
    }

    /// Handle a new WebSocket connection
    pub async fn handle_connection(
        ws: WebSocketUpgrade,
        State(manager): State<Arc<WebSocketManager>>,
    ) -> impl IntoResponse {
        ws.on_upgrade(move |socket| handle_socket(socket, manager))
    }

    /// Send an outbound message to a specific connection
    pub async fn send(&self, msg: OutboundMessage) {
        // The chat_id in OutboundMessage should correspond to our ConnectionId
        let connection_id = &msg.chat_id;

        let connections = self.connections.read().await;
        if let Some(sender) = connections.get(connection_id) {
            let text = msg.content;
            if let Err(e) = sender.send(Message::Text(text.into())) {
                warn!(
                    "Failed to send message to connection {}: {}",
                    connection_id, e
                );
            }
        } else {
            warn!(
                "Connection {} not found for outbound message",
                connection_id
            );
        }
    }
}

async fn handle_socket(socket: WebSocket, manager: Arc<WebSocketManager>) {
    let (mut sender, mut receiver) = socket.split();

    // Create a unique ID for this connection
    let connection_id = uuid::Uuid::new_v4().to_string();
    debug!("New WebSocket connection: {}", connection_id);

    // Create a channel for sending messages to this connection
    let (tx, mut rx) = mpsc::unbounded_channel();

    // Register the connection
    manager
        .connections
        .write()
        .await
        .insert(connection_id.clone(), tx);

    // Spawn a task to forward messages from the channel to the WebSocket
    let mut send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Handle incoming messages
    let mut recv_task = {
        let manager = manager.clone();
        let connection_id = connection_id.clone();
        tokio::spawn(async move {
            while let Some(Ok(msg)) = receiver.next().await {
                match msg {
                    Message::Text(text) => {
                        // Create InboundMessage and send to bus
                        let inbound = InboundMessage {
                            channel: WebSocketChannel,
                            sender_id: "user".to_string(), // In a real app, we might want authentication
                            chat_id: connection_id.clone(),
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
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        })
    };

    // Wait for either task to finish
    tokio::select! {
        _ = (&mut send_task) => {},
        _ = (&mut recv_task) => {},
    }

    // Cleanup
    manager.connections.write().await.remove(&connection_id);
    debug!("WebSocket connection closed: {}", connection_id);
}
