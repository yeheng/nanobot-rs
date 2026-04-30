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

use gasket_types::events::{ChatEvent, InboundMessage, OutboundMessage, SessionKey};

use crate::memory::MemoryBroker;
use crate::types::{BrokerPayload, Envelope, Topic};

/// Abstraction over outbound message delivery.
///
/// Unifies `process_regular` and `process_streaming` so the broker
/// does not duplicate envelope-construction / publish logic.
#[async_trait]
pub trait MessageOutput: Send + Sync {
    async fn send(
        &self,
        msg: OutboundMessage,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

/// [`MessageOutput`] implementation backed by the in-memory broker.
pub struct BrokerOutput {
    broker: Arc<MemoryBroker>,
}

impl BrokerOutput {
    pub fn new(broker: Arc<MemoryBroker>) -> Self {
        Self { broker }
    }
}

#[async_trait]
impl MessageOutput for BrokerOutput {
    async fn send(
        &self,
        msg: OutboundMessage,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.broker
            .publish(Envelope::new(Topic::Outbound, BrokerPayload::Outbound(msg)))
            .await?;
        Ok(())
    }
}

/// Message handler trait — decoupled from AgentLoop.
#[async_trait]
pub trait MessageHandler: Send + Sync {
    async fn handle_message(
        &self,
        session_key: &SessionKey,
        message: &str,
        override_phase: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;

    async fn handle_streaming_message(
        &self,
        message: &str,
        session_key: &SessionKey,
        override_phase: Option<&str>,
    ) -> Result<
        (
            mpsc::Receiver<ChatEvent>,
            tokio::sync::oneshot::Receiver<
                Result<OutboundMessage, Box<dyn std::error::Error + Send + Sync>>,
            >,
        ),
        Box<dyn std::error::Error + Send + Sync>,
    >;

    /// Handle a control command for the session (e.g. force_compact).
    /// Returns a list of ChatEvents to send back to the client.
    async fn handle_command(
        &self,
        session_key: &SessionKey,
        command: &str,
    ) -> Result<Vec<ChatEvent>, Box<dyn std::error::Error + Send + Sync>> {
        let _ = session_key;
        let _ = command;
        Ok(vec![])
    }
}

/// Manages per-session processing tasks.
///
/// Subscribes to `Topic::Inbound` via the broker and dispatches
/// each message to the appropriate session task.
pub struct SessionManager<H: MessageHandler> {
    broker: Arc<MemoryBroker>,
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
    pub fn new(broker: Arc<MemoryBroker>, handler: Arc<H>, idle_timeout: Duration) -> Self {
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
                            match envelope.payload.as_ref() {
                                BrokerPayload::Inbound(msg) => {
                                    self.dispatch_to_session(msg.clone()).await;
                                }
                                other => {
                                    tracing::warn!("SessionManager: unexpected payload on Inbound topic: {:?}", other);
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
    broker: Arc<MemoryBroker>,
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

        // Check for control commands
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&msg.content) {
            if json.get("type").and_then(|v| v.as_str()) == Some("force_compact") {
                match handler.handle_command(&session_key, "force_compact").await {
                    Ok(events) => {
                        for event in events {
                            let mut outbound = OutboundMessage::with_ws_message(
                                msg.channel.clone(),
                                msg.chat_id.clone(),
                                event,
                            );
                            outbound.metadata = msg.metadata.clone();
                            if let Err(e) = broker
                                .publish(Envelope::new(
                                    Topic::Outbound,
                                    BrokerPayload::Outbound(outbound),
                                ))
                                .await
                            {
                                tracing::error!(
                                    "Session [{}] command publish error: {}",
                                    key_str,
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Session [{}] command error: {}", key_str, e);
                    }
                }
                continue;
            }
        }

        let output = BrokerOutput::new(broker.clone());
        let is_streaming = msg.channel.supports_streaming();
        if let Err(e) = process_message(&session_key, msg, &handler, &output).await {
            if is_streaming {
                tracing::error!("Session [{}] streaming error: {}", key_str, e);
            } else {
                tracing::error!("Session [{}] error: {}", key_str, e);
            }
        }
    }
}

async fn process_message<H: MessageHandler + 'static>(
    session_key: &SessionKey,
    msg: InboundMessage,
    handler: &Arc<H>,
    output: &dyn MessageOutput,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if msg.channel.supports_streaming() {
        let channel = msg.channel.clone();
        let chat_id = msg.chat_id.clone();
        let (mut event_rx, result_handle) = handler
            .handle_streaming_message(&msg.content, session_key, msg.override_phase.as_deref())
            .await?;

        // ChatEvent is already a clean WebSocketMessage — no translation needed.
        while let Some(event) = event_rx.recv().await {
            let outbound =
                OutboundMessage::with_ws_message(channel.clone(), chat_id.clone(), event);
            output.send(outbound).await?;
        }

        let _response = result_handle.await??;
    } else {
        let content = handler
            .handle_message(session_key, &msg.content, msg.override_phase.as_deref())
            .await?;
        let mut outbound = OutboundMessage::new(msg.channel, msg.chat_id.clone(), content);
        outbound.metadata = msg.metadata.clone();
        output.send(outbound).await?;
    }
    Ok(())
}
