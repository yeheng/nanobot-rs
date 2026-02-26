## ADDED Requirements

### Requirement: History Summarization via LLM

The system SHALL summarize older conversation messages by calling the existing `provider.chat()` when history exceeds configured limits. When the total token count (via tiktoken-rs) exceeds `token_budget` or the message count exceeds `max_messages`, the system MUST build a `ChatRequest` with the fixed prompt "Summarize the following conversation briefly, keeping key facts." and the messages to be summarized, then call `provider.chat()` to produce a summary.

#### Scenario: Summarization triggered by token budget

- **WHEN** `build_messages()` is called AND the total tiktoken token count of all history messages exceeds `token_budget`
- **THEN** the system SHALL call `provider.chat()` with the older messages (those beyond `recent_keep`) to generate a summary

#### Scenario: Summarization triggered by message count

- **WHEN** `build_messages()` is called AND the number of history messages exceeds `max_messages`
- **THEN** the system SHALL call `provider.chat()` with the older messages to generate a summary

#### Scenario: No summarization needed

- **WHEN** `build_messages()` is called AND history is within both `max_messages` and `token_budget` limits
- **THEN** the system SHALL return all history messages as-is without calling the provider

#### Scenario: Provider error during summarization

- **WHEN** the `provider.chat()` call fails during summarization
- **THEN** the system SHALL log a warning, skip summarization for this turn, and return the full history using the existing token-budget truncation logic as fallback

### Requirement: Summary Persistence

The system SHALL persist generated summaries in the `session_summaries` SQLite table. Each session has at most one summary row. A new summarization replaces the previous summary.

#### Scenario: Summary stored after successful generation

- **WHEN** the provider returns a valid summary
- **THEN** the system SHALL upsert the summary into `session_summaries` (keyed by `session_key`), delete the summarized messages from `session_messages`, and update `last_consolidated` to the index of the last summarized message

#### Scenario: Summary reused on subsequent turns

- **WHEN** a session already has a persisted summary AND history remains within limits
- **THEN** the system SHALL load the existing summary from the database and inject it without calling the provider

#### Scenario: Session cleared

- **WHEN** a session's messages are cleared via `clear_session_messages()`
- **THEN** the system SHALL also delete the associated summary from `session_summaries`

### Requirement: Summary Injection as Assistant Message

The system SHALL inject persisted summaries into the conversation history as an **assistant message** prefixed with `[Conversation Summary]:`. The summary message SHALL appear after the system prompt and before any remaining recent messages.

#### Scenario: History built with summary

- **WHEN** `build_messages()` runs AND the session has a persisted summary
- **THEN** the returned message list SHALL be: `[system prompt] + [assistant: "[Conversation Summary]: {summary}"] + [recent messages] + [current user message]`

#### Scenario: History built without summary

- **WHEN** `build_messages()` runs AND the session has no persisted summary
- **THEN** the returned message list SHALL be: `[system prompt] + [all history messages] + [current user message]` (identical to current behavior)

### Requirement: Async `build_messages()`

`ContextBuilder::build_messages()` SHALL become an `async fn` to support the provider call and SQLite I/O required for summarization. The single call site in `loop_.rs` SHALL add `.await`.

#### Scenario: Call site migration

- **WHEN** `build_messages()` is called from the agent loop
- **THEN** the caller SHALL `.await` the result; the agent loop is already async so this is a mechanical change
