## ADDED Requirements

### Requirement: Per-Session Request Serialization

The system SHALL ensure that requests within the same session (`session_key`) are processed **sequentially** — no two requests for the same session SHALL execute concurrently in the agent loop.

Requests targeting **different** sessions SHALL remain fully concurrent.

#### Scenario: Two messages from same Telegram chat arrive simultaneously
- **WHEN** two inbound messages with session_key `"telegram:12345"` arrive within 100ms of each other
- **THEN** the second message SHALL be queued until the first message's entire `process_direct` pipeline completes
- **AND** both messages SHALL produce correct, non-overlapping session history

#### Scenario: Messages from different sessions are fully concurrent
- **WHEN** message A arrives for session `"telegram:111"` and message B arrives for session `"telegram:222"` simultaneously
- **THEN** both messages SHALL be processed concurrently without blocking each other

### Requirement: Request Identity

The system SHALL assign a unique `request_id` (UUID v4) to each invocation of `process_direct`. This `request_id` SHALL be available as a top-level field in every `*Context` struct passed to hooks throughout the request lifecycle.

#### Scenario: Request ID propagation through hook pipeline
- **WHEN** a message enters `process_direct`
- **THEN** every hook context (`RequestContext`, `SessionLoadContext`, `SessionSaveContext`, `ContextPrepareContext`, `LlmRequestContext`, `LlmResponseContext`, `ToolExecuteContext`, `ToolResultContext`, `ResponseContext`) SHALL contain the same `request_id` value
- **AND** the `request_id` SHALL be a valid UUID v4 string

#### Scenario: Concurrent requests have distinct IDs
- **WHEN** two requests are processed concurrently (different sessions)
- **THEN** each request SHALL have a unique `request_id`

### Requirement: Metadata Continuity

The system SHALL propagate the `metadata` HashMap continuously through the entire request pipeline without gaps. Specifically, metadata written by a hook in any stage SHALL be visible to all subsequent stages within the same request.

#### Scenario: Metadata flows from on_response to final on_session_save
- **WHEN** a hook writes `metadata["key"] = "value"` during `on_response`
- **THEN** the subsequent `on_session_save` (assistant message) SHALL receive a `SessionSaveContext` whose `metadata` contains `"key" → "value"`

#### Scenario: Metadata flows from on_request through entire pipeline
- **WHEN** a hook writes `metadata["trace"] = "abc"` during `on_request`
- **THEN** `on_session_load`, `on_session_save`, `on_context_prepare`, `on_llm_request`, `on_llm_response`, `on_tool_execute`, `on_tool_result`, `on_response`, and the final `on_session_save` SHALL all have access to `"trace" → "abc"` in their metadata
