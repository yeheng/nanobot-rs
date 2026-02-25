## Context

The nanobot project currently uses a synchronous request-response pattern for LLM calls. All providers implement `LlmProvider::chat()` which returns a complete `ChatResponse`. This design was simple to implement but creates poor UX for long responses.

The codebase uses:
- `reqwest` for HTTP client (already supports streaming)
- `tokio` for async runtime
- `serde` for JSON serialization
- Multiple provider implementations via `OpenAICompatibleProvider` and `GeminiProvider`
- Middleware pattern (`LoggingProvider`, `MetricsProvider`, `RateLimitProvider`)

## Goals / Non-Goals

**Goals:**
- Add streaming support for all existing LLM providers
- Maintain backward compatibility with non-streaming mode
- Enable real-time output display in CLI
- Support progressive tool call accumulation
- Keep middleware compatible with streaming

**Non-Goals:**
- Streaming for all channel integrations (Telegram, Discord, etc.) - CLI only initially
- Cancellation support (can be added later)
- Streaming for embedded/file attachments

## Decisions

### Decision 1: Trait Extension Approach

**Chosen:** Add `chat_stream()` method to `LlmProvider` trait with default implementation that falls back to non-streaming.

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    fn default_model(&self) -> &str;
    async fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse>;

    // New method with default fallback
    async fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<ChatStreamChunk>> + Send>>> {
        // Default: convert non-streaming to single-chunk stream
        let response = self.chat(request).await?;
        Ok(futures::stream::once(async move {
            Ok(ChatStreamChunk::from(response))
        }).boxed())
    }
}
```

**Alternatives considered:**
1. **Separate `StreamingLlmProvider` trait**: More complex, requires trait objects for mixed usage
2. **Modify `chat()` to return Stream**: Breaking change for all implementations

### Decision 2: Stream Type

**Chosen:** Use `futures::Stream<Item = anyhow::Result<ChatStreamChunk>>` with `Pin<Box<dyn Stream>>` for trait object safety.

```rust
#[derive(Debug, Clone)]
pub struct ChatStreamChunk {
    pub delta: ChatStreamDelta,
    pub finish_reason: Option<FinishReason>,
}

#[derive(Debug, Clone, Default)]
pub struct ChatStreamDelta {
    pub content: Option<String>,
    pub reasoning_content: Option<String>,
    pub tool_calls: Vec<ToolCallDelta>,
}
```

**Alternatives considered:**
1. **Tokio Stream**: Less standard, harder to compose
2. **Async Iterator**: Not yet stable in Rust

### Decision 3: SSE Implementation

**Chosen:** Use `reqwest` built-in streaming with manual SSE parsing.

```rust
let response = client
    .post(url)
    .json(&request)
    .header("Accept", "text/event-stream")
    .send()
    .await?;

let stream = response.bytes_stream()
    .map_err(|e| anyhow::anyhow!("Stream error: {}", e))
    .and_then(|chunk| parse_sse_chunk(chunk));
```

**Alternatives considered:**
1. **`eventsource-stream` crate**: Additional dependency, may not handle all edge cases
2. **`tokio-stream` wrapper**: Similar complexity

### Decision 4: Middleware Strategy

**Chosen:** Middleware decorates `chat_stream()` similar to `chat()`.

- `LoggingProvider`: Log stream start/end, not each chunk
- `MetricsProvider`: Record stream duration, chunk count
- `RateLimitProvider`: Apply rate limit before stream starts

**Alternatives considered:**
1. **Skip middleware for streaming**: Loses observability
2. **New `StreamingMiddleware` trait**: Too much complexity

## Risks / Trade-offs

| Risk | Mitigation |
|------|------------|
| SSE parsing edge cases | Comprehensive testing with real API responses |
| Memory usage for long streams | Chunk processing, no full buffering |
| Error handling mid-stream | Return error chunk, allow partial recovery |
| Provider-specific streaming formats | Abstract in provider implementations |
| Breaking middleware compatibility | Default trait implementation preserves behavior |

## Migration Plan

1. **Phase 1**: Add types and trait method with default implementation
2. **Phase 2**: Implement streaming for OpenAI-compatible providers
3. **Phase 3**: Implement streaming for Gemini
4. **Phase 4**: Update agent loop with streaming support
5. **Phase 5**: Add CLI flag and integration

No breaking changes required - default implementation ensures backward compatibility.

## Open Questions

1. **Tool call accumulation**: Should we accumulate tool calls in the agent loop or expect complete tool calls per chunk?
   - *Current thinking*: Accumulate in agent loop, as OpenAI streams tool call arguments incrementally

2. **Error recovery**: How to handle partial responses on error?
   - *Current thinking*: Return error chunk, agent loop decides to retry or use partial content
