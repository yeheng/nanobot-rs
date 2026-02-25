## Context

The `process_history()` function in `history_processor.rs` uses a simple token-budget algorithm: take up to `max_messages`, always keep `recent_keep` recent messages verbatim, and drop older messages that exceed the budget. Token estimation uses a naive `text.len() / 3` heuristic that is 30-50% inaccurate (worse for CJK text). When conversations grow beyond `max_messages` (default: 50), older messages are simply dropped — losing important context.

The `sessions.last_consolidated` field already exists in both the SQLite schema and the `SessionMeta` struct, but is never advanced. The project already has `async-trait` as a dependency, and the agent loop is fully async.

**Constraints:**
- Summarization reuses the existing `self.provider.chat()` — no separate provider needed
- `tiktoken-rs` provides accurate BPE token counting for OpenAI-compatible models
- Summaries must be persisted to avoid re-summarizing on every turn
- Must not add latency when no summarization is needed

## Goals / Non-Goals

### Goals
- Use `tiktoken-rs` for accurate BPE token counting, replacing all `text.len() / 3` heuristics
- When history exceeds `max_messages` or `token_budget`, call the existing `provider.chat()` to generate a summary with a fixed prompt: "请简要总结以下对话内容，保留关键事实"
- Persist the summary in a `session_summaries` SQLite table
- Delete the summarized messages from `session_messages`, update `last_consolidated`
- On subsequent turns, load the persisted summary and inject it as an **assistant message** at the beginning of history, before recent messages

### Non-Goals
- Separate summarization provider/model (reuse existing provider)
- Strategy trait / multiple strategies (removed — single linear flow)
- Embedding-based retrieval or RAG
- Streaming summarization progress
- Merging multiple summaries (single summary per session, replaced on each summarization)

## Decisions

### Decision 1: Replace `text.len() / 3` with `tiktoken-rs`

**Why:** The current heuristic is 30-50% off for English and worse for CJK. `tiktoken-rs` provides exact BPE counts via `cl100k_base` encoding (covers GPT-4, GPT-3.5, and most OpenAI-compatible models).

**Integration:**
- Add `tiktoken-rs` dependency to `nanobot-core/Cargo.toml`
- Replace `count_tokens()` in `history_processor.rs` with `tiktoken_rs::cl100k_base()` encoding
- Use a lazy-initialized, cached encoder via `std::sync::OnceLock`
- Fallback: if tiktoken init fails, log warning and use `len() / 4`

### Decision 2: Summarization via existing `provider.chat()`

**Why:** No need for a dedicated summarization provider. The existing provider is already configured and available. Using a fixed Chinese prompt keeps things simple and matches the project's primary user base.

**Flow:**
1. `build_messages()` calculates total history tokens via tiktoken
2. If `token_count > token_budget` or `message_count > max_messages`, trigger summarization
3. Build a `ChatRequest` with system prompt "请简要总结以下对话内容，保留关键事实" and the messages to summarize
4. Call `provider.chat(request).await` → get summary text
5. Write summary to `session_summaries` table
6. Delete the summarized `session_messages` rows
7. Update `last_consolidated` on the session

### Decision 3: Summary injected as assistant message

**Why:** The summary represents conversation history context. Injecting it as an assistant message (prefixed with `[历史对话摘要]:`) at the start of the history keeps the system prompt clean and makes the summary visible as part of the conversation flow. This is more natural for the LLM to consume than embedding it in the system prompt.

**Injection in `build_messages()`:**
```
[system prompt]
[assistant: "[历史对话摘要]: {summary}"]   ← loaded from session_summaries
[remaining recent messages]
[current user message]
```

### Decision 4: Persist summaries in `session_summaries` table

**Why:** Summaries are expensive to compute. Persisting avoids re-summarization.

**Schema:**
```sql
CREATE TABLE session_summaries (
    session_key TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL
);
```

Single row per session — each new summarization replaces the previous one. Simpler than the multi-row indexed approach since we always regenerate a fresh summary.

### Decision 5: `build_messages()` becomes async

**Why:** `build_messages()` now needs to: (1) check if summarization is needed, (2) potentially call `provider.chat()`, (3) read/write SQLite. All three are async operations.

**Migration:**
- `ContextBuilder::build_messages()` → `async fn build_messages()`
- Call site in `loop_.rs` already in async context, just add `.await`
- `ContextBuilder` gains `provider: Arc<dyn LlmProvider>` and `store: Arc<SqliteStore>` fields

## Architecture

```
build_messages() [async]
    │
    ├─ 1. Load existing summary from session_summaries (if any)
    │
    ├─ 2. Count history tokens with tiktoken-rs
    │
    ├─ 3. tokens > budget || messages.len() > max_messages?
    │     │
    │     ├─ NO  → skip to step 5
    │     │
    │     └─ YES → Summarize
    │           ├─ Build ChatRequest with fixed prompt + older messages
    │           ├─ provider.chat(request).await
    │           ├─ Write summary to session_summaries (replace)
    │           ├─ Delete summarized session_messages
    │           └─ Update last_consolidated
    │
    ├─ 4. Load updated summary (if just generated or pre-existing)
    │
    ├─ 5. Build final message list:
    │     [system_prompt]
    │     [assistant: "[历史对话摘要]: {summary}"]  ← if summary exists
    │     [recent messages]                         ← post-consolidated
    │     [current user message]
    │
    └─ Return messages
```

## Risks / Trade-offs

| Risk | Mitigation |
|------|-----------|
| `tiktoken-rs` adds ~2MB binary size | Acceptable for accuracy gain. |
| Summarization adds latency on trigger turn | Only fires when threshold crossed; cached after that. |
| Provider error during summarization | Catch error, log warning, skip summarization, return full history as fallback. |
| Summary loses important details | Keep `recent_keep` messages verbatim; summary is a safety net for old context. |
| SQLite contention from summary writes | WAL mode already enabled; writes are infrequent. |
| `build_messages()` now async | Mechanical change — single call site already in async context. |
