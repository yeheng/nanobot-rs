//! SessionManager — replaces Router Actor + Session Actor.
//!
//! Subscribes to `Topic::Inbound`, dispatches to per-session tasks,
//! preserves serial-per-session processing and idle timeout GC.
//!
//! Defines its own `MessageHandler` trait and `StreamEvent` so
//! gasket-broker has zero dependency on gasket-channels.

use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};

use gasket_types::events::{InboundMessage, OutboundMessage, SessionKey};

use crate::broker::MessageBroker;
use crate::types::{Envelope, Topic};

/// Message handler trait — decoupled from AgentLoop.
#[async_trait]
pub trait MessageHandler: Send + Sync {
    async fn handle_message(
        &self,
        session_key: &SessionKey,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;

    async fn handle_streaming_message(
        &self,
        message: &str,
        session_key: &SessionKey,
    ) -> Result<
        (
            mpsc::Receiver<StreamEvent>,
            tokio::sync::oneshot::Receiver<
                Result<OutboundMessage, Box<dyn std::error::Error + Send + Sync>>,
            >,
        ),
        Box<dyn std::error::Error + Send + Sync>,
    >;
}

/// Stream events — mirrors bus::actors::StreamEvent for broker compatibility.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    Content(String),
    Reasoning(String),
    ToolStart {
        name: String,
        arguments: String,
    },
    ToolEnd {
        name: String,
        output: String,
    },
    Done,
    TokenStats {
        prompt: usize,
        completion: usize,
        total: usize,
    },
}

/// Manages per-session processing tasks.
///
/// Subscribes to `Topic::Inbound` via the broker and dispatches
/// each message to the appropriate session task.
pub struct SessionManager<H: MessageHandler> {
    broker: Arc<dyn MessageBroker>,
    handler: Arc<H>,
    sessions: DashMap<SessionKey, mpsc::Sender<InboundMessage>>,
    idle_timeout: Duration,
}

impl<H: MessageHandler + 'static> Clone for SessionManager<H> {
    fn clone(&self) -> Self {
        Self {
            broker: self.broker.clone(),
            handler: self.handler.clone(),
            sessions: DashMap::new(),
            idle_timeout: self.idle_timeout,
        }
    }
}

impl<H: MessageHandler + 'static> SessionManager<H> {
    pub fn new(broker: Arc<dyn MessageBroker>, handler: Arc<H>, idle_timeout: Duration) -> Self {
        Self {
            broker,
            handler,
            sessions: DashMap::new(),
            idle_timeout,
        }
    }

    /// Main loop — subscribes to Inbound and dispatches messages.
    pub async fn run(self) {
        tracing::info!("SessionManager started");
        let mut sub = match self.broker.subscribe(&Topic::Inbound).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("SessionManager: subscribe failed: {}", e);
                return;
            }
        };
        let mut gc_interval = tokio::time::interval(Duration::from_secs(300));

        loop {
            tokio::select! {
                result = sub.recv() => {
                    match result {
                        Ok(envelope) => {
                            match serde_json::from_value::<InboundMessage>(envelope.payload) {
                                Ok(msg) => self.dispatch_to_session(msg).await,
                                Err(_) => {
                                    tracing::warn!("SessionManager: failed to deserialize InboundMessage");
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("SessionManager: recv error: {}", e);
                            break;
                        }
                    }
                }
                _ = gc_interval.tick() => {
                    let before = self.sessions.len();
                    self.sessions.retain(|key, tx| {
                        let alive = !tx.is_closed();
                        if !alive {
                            tracing::debug!("SessionManager GC: removing dead [{}]", key);
                        }
                        alive
                    });
                    let removed = before - self.sessions.len();
                    if removed > 0 {
                        tracing::info!("SessionManager GC: removed {} sessions", removed);
                    }
                }
            }
        }
        tracing::info!("SessionManager shutting down");
    }

    async fn dispatch_to_session(&self, msg: InboundMessage) {
        let key = msg.session_key().clone();
        let mut needs_respawn = true;

        if let Some(tx) = self.sessions.get(&key) {
            if tx.send(msg.clone()).await.is_ok() {
                needs_respawn = false;
            } else {
                tracing::info!("Session [{}] channel dead, respawning...", key);
            }
        }

        if needs_respawn {
            let (tx, rx) = mpsc::channel(32);
            let broker = self.broker.clone();
            let handler = self.handler.clone();
            let session_key = key.clone();
            let idle_timeout = self.idle_timeout;

            tokio::spawn(async move {
                run_session_task(session_key, rx, broker, handler, idle_timeout).await;
            });

            if let Err(e) = tx.send(msg).await {
                tracing::error!("Failed to send to fresh session [{}]: {}", key, e);
            }
            self.sessions.insert(key, tx);
        }
    }
}

/// Per-session task — serial message processing with idle timeout.
async fn run_session_task<H: MessageHandler + 'static>(
    session_key: SessionKey,
    mut rx: mpsc::Receiver<InboundMessage>,
    broker: Arc<dyn MessageBroker>,
    handler: Arc<H>,
    idle_timeout: Duration,
) {
    let key_str = session_key.to_string();
    tracing::info!("Session task [{}] spawned", key_str);

    loop {
        let msg = match timeout(idle_timeout, rx.recv()).await {
            Ok(Some(msg)) => msg,
            Ok(None) => {
                tracing::info!("Session [{}] channel closed", key_str);
                break;
            }
            Err(_) => {
                tracing::info!("Session [{}] idle timeout", key_str);
                break;
            }
        };

        if msg.channel.supports_streaming() {
            if let Err(e) = process_streaming(&session_key, msg, &handler, &broker).await {
                tracing::error!("Session [{}] streaming error: {}", key_str, e);
            }
        } else if let Err(e) = process_regular(&session_key, msg, &handler, &broker).await {
            tracing::error!("Session [{}] error: {}", key_str, e);
        }
    }
}

async fn process_regular<H: MessageHandler + 'static>(
    session_key: &SessionKey,
    msg: InboundMessage,
    handler: &Arc<H>,
    broker: &Arc<dyn MessageBroker>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let content = handler.handle_message(session_key, &msg.content).await?;
    let outbound = OutboundMessage {
        channel: msg.channel,
        chat_id: msg.chat_id,
        content,
        metadata: None,
        trace_id: msg.trace_id,
        ws_message: None,
    };
    broker
        .publish(Envelope::new(Topic::Outbound, &outbound))
        .await?;
    Ok(())
}

async fn process_streaming<H: MessageHandler + 'static>(
    session_key: &SessionKey,
    msg: InboundMessage,
    handler: &Arc<H>,
    broker: &Arc<dyn MessageBroker>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let channel = msg.channel.clone();
    let chat_id = msg.chat_id.clone();
    let (mut event_rx, result_handle) = handler
        .handle_streaming_message(&msg.content, session_key)
        .await?;

    while let Some(event) = event_rx.recv().await {
        if let Some(ws_msg) = stream_event_to_ws_message(event) {
            let outbound =
                OutboundMessage::with_ws_message(channel.clone(), chat_id.clone(), ws_msg);
            broker
                .publish(Envelope::new(Topic::Outbound, &outbound))
                .await?;
        }
    }
    let _response = result_handle.await??;
    Ok(())
}

fn stream_event_to_ws_message(
    event: StreamEvent,
) -> Option<gasket_types::events::WebSocketMessage> {
    use gasket_types::events::WebSocketMessage;
    match event {
        StreamEvent::Content(c) => Some(WebSocketMessage::content(c)),
        StreamEvent::Reasoning(c) => Some(WebSocketMessage::thinking(c)),
        StreamEvent::ToolStart { name, arguments } => {
            Some(WebSocketMessage::tool_start(name, Some(arguments)))
        }
        StreamEvent::ToolEnd { name, output } => {
            Some(WebSocketMessage::tool_end(name, Some(output)))
        }
        StreamEvent::Done => Some(WebSocketMessage::done()),
        StreamEvent::TokenStats { .. } => None,
    }
}
