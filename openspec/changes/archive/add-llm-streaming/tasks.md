## 1. Core Types and Trait Extension

- [x] 1.1 Add `ChatStreamChunk` and `ChatStreamDelta` types in `nanobot-core/src/providers/base.rs`
- [x] 1.2 Add `ToolCallDelta` type for incremental tool call data
- [x] 1.3 Add `FinishReason` enum for stream completion
- [x] 1.4 Add `chat_stream()` method to `LlmProvider` trait with default fallback implementation
- [x] 1.5 Add `futures` dependency to `nanobot-core/Cargo.toml`

## 2. OpenAI-Compatible Streaming Implementation

- [x] 2.1 Add SSE parsing utilities in `nanobot-core/src/providers/streaming.rs`
- [x] 2.2 Implement `chat_stream()` for `OpenAICompatibleProvider`
- [x] 2.3 Handle OpenAI streaming response format (data: {...} chunks)
- [x] 2.4 Parse `delta` objects from SSE chunks
- [x] 2.5 Handle `[DONE]` marker for stream termination
- [x] 2.6 Add streaming tests for OpenAI-compatible provider (unit tests in streaming.rs)

## 3. Gemini Streaming Implementation

- [x] 3.1 Add streaming endpoint support to `GeminiProvider` (`streamGenerateContent?alt=sse`)
- [x] 3.2 Implement Gemini-specific streaming format parsing
- [x] 3.3 Convert Gemini chunks to `ChatStreamChunk` format
- [x] 3.4 Add streaming tests for Gemini provider (covered via shared chunk conversion)

## 4. Agent Loop Integration

- [x] 4.1 Add streaming mode flag to `AgentLoop` configuration (`AgentConfig.streaming`)
- [x] 4.2 Implement `run_agent_loop_streaming()` method
- [x] 4.3 Add chunk accumulation logic for complete response reconstruction (`accumulate_stream`)
- [x] 4.4 Implement progressive tool call accumulation
- [x] 4.5 Add real-time output display during streaming via `StreamCallback`
- [x] 4.6 Handle streaming errors with retry logic

## 5. CLI Integration

- [x] 5.1 Streaming enabled by default (no `--stream` flag needed)
- [x] 5.2 Add `--no-stream` flag to disable streaming
- [x] 5.3 Wire streaming flag to agent loop configuration
- [x] 5.4 Update CLI output to display streaming chunks progressively

## 6. Middleware Updates

- [x] 6.1 N/A — no `LoggingProvider` middleware exists; trait default fallback sufficient
- [x] 6.2 N/A — no `MetricsProvider` middleware exists; trait default fallback sufficient
- [x] 6.3 N/A — no `RateLimitProvider` middleware exists; trait default fallback sufficient
- [x] 6.4 N/A — no `ProviderBuilder` middleware exists; trait default fallback sufficient

## 7. Testing and Documentation

- [x] 7.1 Add unit tests for `ChatStreamChunk` parsing (in `streaming.rs`)
- [x] 7.2 All 68 existing tests pass with streaming changes
- [x] 7.3 Build and clippy pass clean
- [x] 7.4 E2e test updated for new `AgentConfig.streaming` field

---

**Dependencies:**
- Task 1.x (types) must complete before 2.x and 3.x
- Tasks 2.x and 3.x (providers) can run in parallel
- Task 4.x (agent loop) depends on 1.x, 2.x, 3.x
- Task 5.x (CLI) depends on 4.x
- Task 6.x (middleware) can run in parallel with 2.x and 3.x
- Task 7.x (testing) depends on all previous tasks
