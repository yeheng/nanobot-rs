# LLM Streaming Capability

## ADDED Requirements

### Requirement: Streaming Chat Interface

The LLM provider trait SHALL support streaming responses via a `chat_stream()` method that returns an async stream of response chunks.

#### Scenario: Successful streaming response
- **WHEN** a client calls `chat_stream()` with a valid chat request
- **THEN** the provider returns a stream that yields `ChatStreamChunk` items progressively

#### Scenario: Streaming fallback for non-streaming providers
- **WHEN** a provider does not natively support streaming
- **THEN** the default implementation converts the non-streaming response to a single-chunk stream

#### Scenario: Stream error handling
- **WHEN** an error occurs mid-stream
- **THEN** the stream yields an error item and terminates gracefully

---

### Requirement: Stream Chunk Data Structure

The system SHALL provide a `ChatStreamChunk` type containing incremental response data including content deltas, reasoning content, and tool call deltas.

#### Scenario: Content delta chunk
- **WHEN** the LLM generates text content incrementally
- **THEN** each chunk contains the new text in `delta.content` field

#### Scenario: Reasoning content chunk
- **WHEN** the LLM generates chain-of-thought reasoning (e.g., DeepSeek thinking mode)
- **THEN** reasoning text is provided in `delta.reasoning_content` field

#### Scenario: Tool call delta chunk
- **WHEN** the LLM streams tool call arguments incrementally
- **THEN** each chunk contains partial tool call data in `delta.tool_calls`

#### Scenario: Stream completion
- **WHEN** the LLM finishes generating
- **THEN** the final chunk includes `finish_reason` indicating completion

---

### Requirement: OpenAI-Compatible Streaming

OpenAI-compatible providers (OpenAI, Anthropic, OpenRouter, DashScope, Moonshot, Zhipu, MiniMax, DeepSeek, Ollama) SHALL implement SSE-based streaming following the OpenAI API format.

#### Scenario: SSE stream connection
- **WHEN** `chat_stream()` is called on an OpenAI-compatible provider
- **THEN** the request includes `Accept: text/event-stream` header and `stream: true` in the body

#### Scenario: SSE chunk parsing
- **WHEN** an SSE data chunk is received
- **THEN** the system parses the JSON delta and yields a `ChatStreamChunk`

#### Scenario: SSE stream termination
- **WHEN** the SSE stream sends `[DONE]` marker
- **THEN** the stream completes successfully

---

### Requirement: Gemini Streaming

The Gemini provider SHALL implement streaming using Google's native streaming API format.

#### Scenario: Gemini stream request
- **WHEN** `chat_stream()` is called on the Gemini provider
- **THEN** the request uses the streaming endpoint with appropriate parameters

#### Scenario: Gemini chunk conversion
- **WHEN** a Gemini streaming response chunk is received
- **THEN** the system converts it to the standard `ChatStreamChunk` format

---

### Requirement: Agent Loop Streaming Integration

The agent loop SHALL support streaming mode that displays content progressively while accumulating the complete response.

#### Scenario: Streaming enabled agent loop
- **WHEN** streaming is enabled and the agent makes an LLM call
- **THEN** content chunks are displayed in real-time to the user

#### Scenario: Streaming response accumulation
- **WHEN** streaming mode processes chunks
- **THEN** the system accumulates all content into a complete `ChatResponse` for subsequent processing

#### Scenario: Tool call accumulation during streaming
- **WHEN** tool calls are streamed incrementally
- **THEN** the system accumulates partial tool call deltas into complete `ToolCall` objects

---

### Requirement: CLI Streaming Control

The CLI SHALL provide a flag to control streaming behavior with streaming enabled by default.

#### Scenario: Default streaming behavior
- **WHEN** the user runs nanobot without streaming flags
- **THEN** streaming mode is enabled by default

#### Scenario: Disable streaming
- **WHEN** the user provides `--no-stream` flag
- **THEN** the agent uses non-streaming `chat()` method

#### Scenario: Explicit streaming enable
- **WHEN** the user provides `--stream` flag
- **THEN** streaming mode is explicitly enabled
