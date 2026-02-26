## Phase 1: tiktoken-rs Integration

- [x] 1.1 Add `tiktoken-rs` dependency to `nanobot-core/Cargo.toml`
- [x] 1.2 Replace `count_tokens()` in `history_processor.rs` with tiktoken-rs `cl100k_base` encoding, using `std::sync::OnceLock` for lazy init
- [x] 1.3 Add fallback: if tiktoken init fails, log warning and use `text.len() / 4`
- [x] 1.4 Run `cargo test` and `cargo clippy` — verify no regressions

## Phase 2: SQLite Summary Storage

- [x] 2.1 Add `session_summaries` table to `SqliteStore::init_db()`: `(session_key TEXT PRIMARY KEY, content TEXT NOT NULL, created_at TEXT NOT NULL)`
- [x] 2.2 Add `save_session_summary(session_key, content)` — upsert single row
- [x] 2.3 Add `load_session_summary(session_key) -> Option<String>` — load summary
- [x] 2.4 Add `delete_session_summary(session_key)` — delete summary
- [x] 2.5 Update `clear_session_messages()` to also call `delete_session_summary()`
- [x] 2.6 Write unit tests for all new SQLite methods

## Phase 3: Async `build_messages()` + Summarization

- [x] 3.1 Add `provider: Option<Arc<dyn LlmProvider>>`, `store: Option<Arc<SqliteStore>>`, and `model: Option<String>` fields to `ContextBuilder`; add `with_summarization()` builder method
- [x] 3.2 Add `session_key: &str` parameter to `build_messages()`
- [x] 3.3 Convert `build_messages()` to `async fn`
- [x] 3.4 At the start of `build_messages()`: load existing summary via `store.load_session_summary()`
- [x] 3.5 After `process_history()`: check if `filtered_count > 0` (messages were dropped); if yes and summarization is configured, build `ChatRequest` with fixed prompt + context, call `provider.chat().await`
- [x] 3.6 On successful summary: call `store.save_session_summary()` to persist
- [x] 3.7 On provider error: log warning, skip summarization, continue with existing summary or truncated history
- [x] 3.8 Inject summary as assistant message: `ChatMessage::assistant("[Conversation Summary]: {summary}")` after system prompt, before recent messages
- [x] 3.9 Update call site in `loop_.rs` to `.await` on `build_messages()` and pass `session_key`
- [x] 3.10 Wire up `with_summarization()` in `AgentLoop::new()` to enable summarization by default

## Phase 4: Testing & Validation

- [x] 4.1 Unit test: tiktoken `count_tokens()` returns accurate BPE counts for known English/Chinese strings
- [x] 4.2 Unit test: `save_session_summary` / `load_session_summary` / `delete_session_summary` CRUD
- [x] 4.3 Unit test: `clear_session_messages()` also clears summary
- [x] 4.4 Unit test: summary upsert replaces previous value
- [x] 4.5 Run `cargo test` — full suite (140 unit + 68 e2e), no regressions
- [x] 4.6 Run `cargo clippy` — no new warnings
