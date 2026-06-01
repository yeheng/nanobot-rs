//! Request execution pipeline — extracted from `AgentSession`.
//!
//! Per Linus review: `AgentSession` was carrying both **session lifecycle**
//! (list/clear/compact/shutdown) and **per-turn request orchestration**
//! (preprocess → kernel → postprocess). Those are different lifetimes and
//! different responsibilities. This module owns the request-orchestration
//! half; the facade in `session/mod.rs` keeps the lifecycle half.
//!
//! `RequestPipeline` is intentionally small: it holds only what every turn
//! needs (the finalizer + a TaskTracker for graceful shutdown). All other
//! state — runtime_ctx, context_builder, compactor, active model — is
//! threaded in as method arguments, so the pipeline never owns anything
//! that should belong to the session.

use std::sync::Arc;

use futures_util::StreamExt;
use gasket_providers::ChatMessage;
use gasket_types::events::ChatEvent;
use gasket_types::SessionKey;
use tokio_stream::wrappers::ReceiverStream;

use crate::error::AgentError;
use crate::kernel::{self, ExecutionResult, RuntimeContext, StreamEvent};
use crate::session::compactor::ContextCompactor;
use crate::session::finalizer::ResponseFinalizer;
use crate::session::history::builder::{BuildOutcome, ContextBuilder};
use crate::session::{AgentResponse, FinalizeContext};

/// Per-turn context carried through preprocess → execute → postprocess.
pub(crate) struct PipelineContext {
    pub(crate) runtime_ctx: RuntimeContext,
    pub(crate) messages: Vec<ChatMessage>,
    pub(crate) fctx: FinalizeContext,
    pub(crate) model: String,
    pub(crate) finalizer: ResponseFinalizer,
}

/// Request-execution pipeline owned by `AgentSession`.
///
/// The pipeline does NOT own runtime context, history, or compaction state —
/// those belong to the session lifecycle. It only holds the finalizer that
/// every turn invokes after the kernel returns, plus a TaskTracker so the
/// session can await in-flight finalization on graceful shutdown.
pub(crate) struct RequestPipeline {
    finalizer: ResponseFinalizer,
    pending_done: tokio_util::task::TaskTracker,
}

impl RequestPipeline {
    pub(crate) fn new(
        finalizer: ResponseFinalizer,
        pending_done: tokio_util::task::TaskTracker,
    ) -> Self {
        Self {
            finalizer,
            pending_done,
        }
    }

    pub(crate) fn pending_done(&self) -> &tokio_util::task::TaskTracker {
        &self.pending_done
    }

    /// Stage 1: PreProcess — build request, wire checkpoint callback.
    pub(crate) async fn preprocess(
        &self,
        runtime_ctx_template: &RuntimeContext,
        active_model: String,
        context_builder: &ContextBuilder,
        compactor: Option<&Arc<ContextCompactor>>,
        content: &str,
        session_key: &SessionKey,
    ) -> Result<(PipelineContext, Option<String>), AgentError> {
        let outcome = context_builder.build(content, session_key).await?;
        let request = match outcome {
            BuildOutcome::Aborted(msg) => {
                let ctx = PipelineContext {
                    runtime_ctx: runtime_ctx_template.clone(),
                    messages: vec![],
                    fctx: FinalizeContext::new(session_key, content),
                    model: active_model.clone(),
                    finalizer: self.finalizer.clone(),
                };
                return Ok((ctx, Some(msg)));
            }
            BuildOutcome::Ready(req) => req,
        };

        let fctx = FinalizeContext::from_request(&request);
        let messages = request.messages;
        let mut runtime_ctx = runtime_ctx_template.clone();
        runtime_ctx.refs.session_key = Some(session_key.clone());

        if let Some(compactor) = compactor {
            runtime_ctx.checkpoint_callback =
                Some(Arc::new(super::SessionCheckpointCallback::new(
                    fctx.session_key.clone(),
                    compactor.clone(),
                    context_builder.event_store().clone(),
                )));
        }

        let ctx = PipelineContext {
            runtime_ctx,
            messages,
            fctx,
            model: active_model,
            finalizer: self.finalizer.clone(),
        };
        Ok((ctx, None))
    }

    /// Stage 2: Execute — run the kernel streaming loop.
    async fn execute(
        runtime_ctx: &RuntimeContext,
        messages: Vec<ChatMessage>,
        kernel_tx: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<ExecutionResult, AgentError> {
        match kernel::execute_streaming(runtime_ctx, messages, kernel_tx).await {
            Ok(r) => Ok(r),
            Err(crate::kernel::KernelError::MaxIterations(n)) => Ok(ExecutionResult {
                content: format!("Maximum iterations ({}) reached.", n),
                reasoning_content: None,
                tools_used: vec![],
                token_usage: None,
            }),
            Err(e) => Err(e.into()),
        }
    }

    /// Stage 3: PostProcess — finalize response, persist events, trigger compaction.
    async fn postprocess(result: ExecutionResult, ctx: &PipelineContext) -> AgentResponse {
        ctx.finalizer.finalize(result, &ctx.fctx, &ctx.model).await
    }

    /// Spawn the kernel + stream-forwarding pipeline as a tracked task.
    pub(crate) fn spawn_pipeline_task(
        &self,
        ctx: PipelineContext,
        messages: Vec<ChatMessage>,
        kernel_tx: tokio::sync::mpsc::Sender<StreamEvent>,
        kernel_rx: tokio::sync::mpsc::Receiver<StreamEvent>,
        chat_tx: tokio::sync::mpsc::Sender<ChatEvent>,
    ) -> tokio::task::JoinHandle<Result<AgentResponse, AgentError>> {
        let chat_tx_err = chat_tx.clone();
        self.pending_done.spawn(async move {
            let stream_future = ReceiverStream::new(kernel_rx)
                .filter_map(|event| futures_util::future::ready(event.to_chat_event()))
                .for_each(|chat| {
                    let chat_tx = chat_tx.clone();
                    async move {
                        let _ = chat_tx.send(chat).await;
                    }
                });

            let exec_future = Self::execute(&ctx.runtime_ctx, messages, kernel_tx);
            let (result, _) = tokio::join!(exec_future, stream_future);

            if let Err(ref e) = result {
                let _ = chat_tx_err
                    .send(ChatEvent::error(format!("Agent error: {}", e)))
                    .await;
                let _ = chat_tx_err.send(ChatEvent::done()).await;
            }

            let result = result?;
            let response = Self::postprocess(result, &ctx).await;

            Ok(response)
        })
    }
}

/// Construct the response pair for the early-abort path (BeforeRequest hook
/// aborted the pipeline). No kernel is invoked; just emits the abort message.
#[allow(dead_code)]
pub(crate) fn early_abort_response(
    msg: String,
    model: String,
) -> (
    tokio::sync::mpsc::Receiver<ChatEvent>,
    tokio::task::JoinHandle<Result<AgentResponse, AgentError>>,
) {
    let (_tx, rx) = tokio::sync::mpsc::channel(1);
    let handle = tokio::spawn(async move {
        Ok(AgentResponse {
            content: msg,
            reasoning_content: None,
            tools_used: vec![],
            model: Some(model),
            token_usage: None,
            cost: 0.0,
        })
    });
    (rx, handle)
}

/// Spawn a bridge task: every `OutboundMessage::Stream` payload is forwarded
/// as a `ChatEvent` onto `chat_tx`. Returns the sender for tools to use.
#[allow(dead_code)]
pub(crate) fn bridge_outbound_to_chat(
    chat_tx: tokio::sync::mpsc::Sender<ChatEvent>,
) -> tokio::sync::mpsc::Sender<gasket_types::events::OutboundMessage> {
    let (outbound_tx, mut outbound_rx) =
        tokio::sync::mpsc::channel::<gasket_types::events::OutboundMessage>(64);
    tokio::spawn(async move {
        while let Some(msg) = outbound_rx.recv().await {
            if let gasket_types::events::OutboundPayload::Stream(chat_event) = msg.payload {
                let _ = chat_tx.send(chat_event).await;
            }
        }
    });
    outbound_tx
}
