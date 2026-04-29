//! SynthesisCallback implementation for subagent result aggregation.

use std::sync::Arc;

use gasket_providers::{ChatMessage, ChatRequest, LlmProvider};
use gasket_types::{
    events::{ChatEvent, OutboundMessage, SessionKey},
    SubagentResult, SynthesisCallback,
};
use tokio::sync::mpsc;
use tracing::info;

pub struct WebSocketSynthesizer {
    provider: Arc<dyn LlmProvider>,
    model: String,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    session_key: SessionKey,
}

impl WebSocketSynthesizer {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        model: String,
        outbound_tx: mpsc::Sender<OutboundMessage>,
        session_key: SessionKey,
    ) -> Self {
        Self {
            provider,
            model,
            outbound_tx,
            session_key,
        }
    }
}

impl SynthesisCallback for WebSocketSynthesizer {
    fn synthesize(
        &self,
        results: Vec<SubagentResult>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send>>> + Send>,
    > {
        let provider = self.provider.clone();
        let model = self.model.clone();
        let outbound_tx = self.outbound_tx.clone();
        let session_key = self.session_key.clone();

        Box::pin(async move {
            info!("[Synthesizer] Synthesizing {} subagent results", results.len());

            let mut prompt = format!(
                "以下是 {} 个并行任务的结果，请综合分析并给出最终回复：\n\n",
                results.len()
            );
            for (idx, result) in results.iter().enumerate() {
                prompt.push_str(&format!("## Task {}\n", idx + 1));
                prompt.push_str(&format!("**任务**: {}\n", result.task));
                if result.response.content.starts_with("Error:") {
                    prompt.push_str(&format!(
                        "**结果**: [错误] {}\n\n",
                        result.response.content
                    ));
                } else {
                    prompt.push_str(&format!("**结果**: {}\n\n", result.response.content));
                }
            }
            prompt.push_str("请基于以上结果，给出综合性的最终回复。");

            let request = ChatRequest {
                model: model.clone(),
                messages: vec![ChatMessage::user(&prompt)],
                tools: None,
                temperature: None,
                max_tokens: None,
                thinking: None,
            };

            let response = provider
                .chat(request)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;

            if let Some(ref reasoning) = response.reasoning_content {
                let msg = OutboundMessage::with_ws_message(
                    session_key.channel.clone(),
                    session_key.chat_id.clone(),
                    ChatEvent::thinking(reasoning),
                );
                let _ = outbound_tx.send(msg).await;
            }

            let content = response.content.unwrap_or_default();
            let msg = OutboundMessage::with_ws_message(
                session_key.channel.clone(),
                session_key.chat_id.clone(),
                ChatEvent::content(&content),
            );
            let _ = outbound_tx.send(msg).await;

            let msg = OutboundMessage::with_ws_message(
                session_key.channel,
                session_key.chat_id,
                ChatEvent::done(),
            );
            let _ = outbound_tx.send(msg).await;

            Ok(())
        })
    }
}
