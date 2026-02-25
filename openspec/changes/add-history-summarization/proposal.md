# Change: Implement History Summarization with tiktoken-rs

## Why

The `process_history()` function in `history_processor.rs` drops older messages when conversations exceed `max_messages` (50) or `token_budget` — losing important context silently. Token estimation uses a naive `text.len() / 3` heuristic that is 30-50% inaccurate, especially for CJK text. There is no mechanism to preserve old context in a compact form.

## What Changes

- **tiktoken-rs token counting**: Replace `count_tokens()` with accurate BPE counting via `tiktoken-rs` (`cl100k_base` encoding). Fallback to `len() / 4` if initialization fails.
- **LLM summarization**: When history exceeds `max_messages` or `token_budget`, call the existing `provider.chat()` with a fixed prompt ("Summarize the following conversation briefly, keeping key facts.") to generate a summary of older messages.
- **Summary persistence**: New `session_summaries` SQLite table (single row per session). After summarization, delete the summarized `session_messages` rows and update `last_consolidated`.
- **Summary injection**: On subsequent turns, load the persisted summary and inject it as an **assistant message** prefixed with `[Conversation Summary]:` at the start of the history, before recent messages.
- **Async `build_messages()`**: `ContextBuilder::build_messages()` becomes `async fn` to support provider calls and SQLite I/O. Single call site in `loop_.rs` adds `.await`.

## Impact

- Affected specs: `history-summarization` (new), `token-estimation` (new)
- Affected code:
  - `nanobot-core/src/agent/history_processor.rs` — `count_tokens()` replaced with tiktoken-rs; summarization trigger logic added
  - `nanobot-core/src/agent/context.rs` — `build_messages()` becomes async; gains provider/store fields; summary injection as assistant message
  - `nanobot-core/src/agent/loop_.rs` — `.await` added to `build_messages()` call
  - `nanobot-core/src/memory/sqlite.rs` — `session_summaries` table and CRUD methods added
  - `nanobot-core/Cargo.toml` — `tiktoken-rs` dependency added
- **No breaking trait changes**: No strategy trait refactor. The existing `process_history()` function is updated in-place.
- Backward compatible: summarization only triggers when thresholds are exceeded. Existing short conversations behave identically.
