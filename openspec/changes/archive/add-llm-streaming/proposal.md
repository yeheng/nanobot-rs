# Change: Add LLM Streaming Support

## Why

Currently, the nanobot LLM integration only supports synchronous request-response patterns, requiring users to wait for complete responses before seeing any output. This creates poor user experience, especially for long-form responses, as users receive no feedback during generation. Streaming support is essential for:

1. **Real-time feedback**: Users see content as it's generated, improving perceived latency
2. **Better UX**: Progressive output creates a more interactive experience
3. **Cancellable operations**: Streaming enables early cancellation of long-running requests
4. **Industry standard**: Most modern AI assistants support streaming responses

## What Changes

- **ADDED** `LlmProvider::chat_stream()` trait method for streaming responses
- **ADDED** `ChatStreamChunk` type for incremental response data
- **ADDED** Streaming implementation for OpenAI-compatible providers (OpenAI, Anthropic, OpenRouter, etc.)
- **ADDED** Streaming implementation for Gemini provider
- **MODIFIED** Agent loop to support streaming mode
- **ADDED** CLI flag `--stream` (default: enabled) to toggle streaming

## Impact

- **Affected specs**: llm-streaming (new capability)
- **Affected code**:
  - `nanobot-core/src/llm/provider.rs` - Add streaming trait method
  - `nanobot-core/src/llm/types.rs` - Add streaming types
  - `nanobot-core/src/llm/providers/openai_compatible.rs` - Implement SSE streaming
  - `nanobot-core/src/llm/providers/gemini.rs` - Implement Gemini streaming
  - `nanobot-core/src/agent/loop.rs` - Integrate streaming in agent loop
  - `nanobot-cli/src/main.rs` - Add streaming CLI flag
- **Dependencies**: May need `futures` crate for Stream trait, `eventsource-stream` or similar for SSE parsing
- **Backward compatibility**: Non-streaming mode remains available via `--no-stream` flag
