## Phase 1: tiktoken-rs Integration

- [ ] 1.1 Add `tiktoken-rs` dependency to `nanobot-core/Cargo.toml`
- [ ] 1.2 Replace `count_tokens()` in `history_processor.rs` with tiktoken-rs `cl100k_base` encoding, using `std::sync::OnceLock` for lazy init
- [ ] 1.3 Add fallback: if tiktoken init fails, log warning and use `text.len() / 4`
- [ ] 1.4 Run `cargo test` and `cargo clippy` — verify no regressions

## Phase 2: SQLite Summary Storage

- [ ] 2.1 Add `session_summaries` table to `SqliteStore::init_db()`: `(session_key TEXT PRIMARY KEY, content TEXT NOT NULL, created_at TEXT NOT NULL)`
- [ ] 2.2 Add `save_session_summary(session_key, content)` — upsert single row
- [ ] 2.3 Add `load_session_summary(session_key) -> Option<String>` — load summary
- [ ] 2.4 Add `delete_session_summary(session_key)` — delete summary
- [ ] 2.5 Update `clear_session_messages()` to also call `delete_session_summary()`
- [ ] 2.6 Write unit tests for all new SQLite methods

## Phase 3: Async `build_messages()` + Summarization

- [ ] 3.1 Add `provider: Arc<dyn LlmProvider>` and `store: Arc<SqliteStore>` fields to `ContextBuilder`; update constructor
- [ ] 3.2 Add `session_key: String` parameter to `build_messages()`
- [ ] 3.3 Convert `build_messages()` to `async fn`
- [ ] 3.4 At the start of `build_messages()`: load existing summary via `store.load_session_summary()`
- [ ] 3.5 After `process_history()`: check if `token_count > token_budget || messages.len() > max_messages`; if yes, build `ChatRequest` with fixed prompt + older messages, call `provider.chat().await`
- [ ] 3.6 On successful summary: call `store.save_session_summary()`, delete summarized `session_messages`, update `last_consolidated`
- [ ] 3.7 On provider error: log warning, skip summarization, continue with truncated history
- [ ] 3.8 Inject summary as assistant message: `ChatMessage::assistant("[历史对话摘要]: {summary}")` after system prompt, before recent messages
- [ ] 3.9 Update call site in `loop_.rs` to `.await` on `build_messages()`

## Phase 4: Testing & Validation

- [ ] 4.1 Unit test: tiktoken `count_tokens()` returns accurate BPE counts for known English/Chinese strings
- [ ] 4.2 Unit test: `build_messages()` injects summary as assistant message when summary exists in DB
- [ ] 4.3 Unit test: `build_messages()` returns normal messages when no summary and within limits
- [ ] 4.4 Unit test: `clear_session_messages()` also clears summary
- [ ] 4.5 Run `cargo test` — full suite, no regressions
- [ ] 4.6 Run `cargo clippy` — no new warnings
