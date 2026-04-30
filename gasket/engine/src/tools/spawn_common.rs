//! Shared logic for spawn and spawn_parallel tools

use std::sync::Arc;

use gasket_types::{
    events::{ChatEvent, OutboundMessage, SessionKey},
    SubagentResult, SynthesisCallback,
};
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Context for aggregator WebSocket output.
pub struct AggregatorContext {
    pub session_key: SessionKey,
    pub outbound_tx: mpsc::Sender<OutboundMessage>,
    pub ws_summary_limit: usize,
}

/// Forward subagent StreamEvents to WebSocket as ChatEvents.
pub fn spawn_event_forwarder(
    subagent_id: String,
    mut event_rx: mpsc::Receiver<gasket_types::StreamEvent>,
    session_key: SessionKey,
    outbound_tx: mpsc::Sender<OutboundMessage>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        use gasket_types::StreamEventKind;
        while let Some(event) = event_rx.recv().await {
            let chat_event = match &event.kind {
                StreamEventKind::Thinking { content } => {
                    Some(ChatEvent::subagent_thinking(&subagent_id, content.as_ref()))
                }
                StreamEventKind::ToolStart { name, arguments } => {
                    Some(ChatEvent::subagent_tool_start(
                        &subagent_id,
                        name.as_ref(),
                        arguments.as_ref().map(|s| s.to_string()),
                    ))
                }
                StreamEventKind::ToolEnd { name, output } => Some(ChatEvent::subagent_tool_end(
                    &subagent_id,
                    name.as_ref(),
                    output.as_ref().map(|s| s.to_string()),
                )),
                StreamEventKind::Content { content } => {
                    Some(ChatEvent::subagent_content(&subagent_id, content.as_ref()))
                }
                _ => None,
            };
            if let Some(chat_event) = chat_event {
                let msg = OutboundMessage::with_ws_message(
                    session_key.channel.clone(),
                    session_key.chat_id.clone(),
                    chat_event,
                );
                let _ = outbound_tx.send(msg).await;
            }
        }
    })
}

/// Send a ChatEvent to WebSocket via outbound_tx.
pub async fn send_ws_event(
    session_key: &SessionKey,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    event: ChatEvent,
) {
    let msg = OutboundMessage::with_ws_message(
        session_key.channel.clone(),
        session_key.chat_id.clone(),
        event,
    );
    let _ = outbound_tx.send(msg).await;
}

/// Send startup events synchronously for all spawned tasks.
/// Must be called before returning from execute() to guarantee ordering.
pub async fn send_startup_events(
    session_key: &SessionKey,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    count: usize,
    tasks: &[(String, String, u32)],
) {
    send_ws_event(
        session_key,
        outbound_tx,
        ChatEvent::subagent_all_started(count as u32),
    )
    .await;
    for (id, task, index) in tasks {
        send_ws_event(
            session_key,
            outbound_tx,
            ChatEvent::subagent_started(id, task, *index),
        )
        .await;
    }
}

/// Spawn the Aggregator background task.
pub fn spawn_aggregator(
    result_receivers: Vec<tokio::sync::oneshot::Receiver<SubagentResult>>,
    subagent_ids: Vec<String>,
    subagent_indices: Vec<u32>,
    synthesis_callback: Arc<dyn SynthesisCallback>,
    cancellation_token: tokio_util::sync::CancellationToken,
    ctx: AggregatorContext,
) -> tokio::task::JoinHandle<()> {
    let AggregatorContext {
        session_key,
        outbound_tx,
        ws_summary_limit,
    } = ctx;
    tokio::spawn(async move {
        // Clone for the cancellation branch since tokio::select! may move values
        let cancel_ids = subagent_ids.clone();
        let cancel_indices = subagent_indices.clone();

        let results = tokio::select! {
            results = collect_all_results(
                result_receivers,
                subagent_ids,
                subagent_indices,
                &session_key,
                &outbound_tx,
                ws_summary_limit,
            ) => results,
            _ = cancellation_token.cancelled() => {
                info!("[Aggregator] Cancelled");
                // Notify frontend that all subagents are cancelled so they don't stay in "running" state
                for (i, id) in cancel_ids.iter().enumerate() {
                    let index = cancel_indices.get(i).copied().unwrap_or(0);
                    send_ws_event(
                        &session_key,
                        &outbound_tx,
                        ChatEvent::subagent_error(id, index, "Cancelled"),
                    )
                    .await;
                }
                return;
            }
        };

        send_ws_event(
            &session_key,
            &outbound_tx,
            ChatEvent::subagent_synthesizing(),
        )
        .await;

        match synthesis_callback.synthesize(results).await {
            Ok(()) => {}
            Err(e) => {
                warn!("[Aggregator] Synthesis failed: {}", e);
                send_ws_event(
                    &session_key,
                    &outbound_tx,
                    ChatEvent::error(format!("Synthesis failed: {}", e)),
                )
                .await;
                send_ws_event(&session_key, &outbound_tx, ChatEvent::done()).await;
            }
        }
    })
}

async fn collect_all_results(
    receivers: Vec<tokio::sync::oneshot::Receiver<SubagentResult>>,
    subagent_ids: Vec<String>,
    subagent_indices: Vec<u32>,
    session_key: &SessionKey,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    ws_summary_limit: usize,
) -> Vec<SubagentResult> {
    let per_task_timeout = std::time::Duration::from_secs(600);
    let mut results = Vec::with_capacity(receivers.len());
    for (i, rx) in receivers.into_iter().enumerate() {
        let id = subagent_ids.get(i).map(|s| s.as_str()).unwrap_or("unknown");
        let index = subagent_indices.get(i).copied().unwrap_or(0);
        match tokio::time::timeout(per_task_timeout, rx).await {
            Ok(Ok(result)) => {
                let summary = if ws_summary_limit == 0 {
                    result.response.content.clone()
                } else {
                    result
                        .response
                        .content
                        .chars()
                        .take(ws_summary_limit)
                        .collect()
                };
                send_ws_event(
                    session_key,
                    outbound_tx,
                    ChatEvent::subagent_completed(
                        id,
                        index,
                        &summary,
                        result.response.tools_used.len() as u32,
                    ),
                )
                .await;
                results.push(result);
            }
            Ok(Err(_)) => {
                warn!("[Aggregator] Subagent {} result channel closed", id);
                send_ws_event(
                    session_key,
                    outbound_tx,
                    ChatEvent::subagent_error(id, index, "Result channel closed"),
                )
                .await;
            }
            Err(_) => {
                warn!("[Aggregator] Subagent {} timed out", id);
                send_ws_event(
                    session_key,
                    outbound_tx,
                    ChatEvent::subagent_error(id, index, "Subagent timed out"),
                )
                .await;
            }
        }
    }
    results
}
