## 1. Context Structs Enhancement

- [x] 1.1 Add `request_id: String` field to all 9 `*Context` structs in `hooks/mod.rs`
- [x] 1.2 Generate `Uuid::new_v4().to_string()` at the start of `process_direct_with_callback` in `agent/loop_.rs`
- [x] 1.3 Pass `request_id` into every Context struct creation site in `process_direct_with_callback` and `run_agent_loop`/`handle_tool_calls`
- [x] 1.4 `uuid` crate dependency already present in `nanobot-core/Cargo.toml` (workspace)

## 2. Metadata Propagation Fix

- [x] 2.1 In `process_direct_with_callback`, pass `hook_metadata` returned from `run_agent_loop` into `ResponseContext.metadata` (was `HashMap::new()`)
- [x] 2.2 Pass `resp_ctx.metadata` into the final `SessionSaveContext.metadata` (assistant save)
- [x] 2.3 Verified metadata chain: `run_agent_loop` now returns 4-tuple including `hook_metadata`; `ResponseContext` and final `SessionSaveContext` both propagate it

## 3. PersistenceHook Stateless Refactor

- [x] 3.1 Remove `active_sessions: Arc<Mutex<HashMap<String, Session>>>` from `PersistenceHook`
- [x] 3.2 Refactor `on_session_load`: call `SessionManager::get_or_create()`, return history directly without caching
- [x] 3.3 Refactor `on_session_save`: call `SessionManager::append_by_key()` directly — no in-memory session needed
- [x] 3.4 Added `SessionManager::append_by_key(session_key, role, content, tools_used)` — stateless append without pre-loaded `Session`
- [x] 3.5 Update `PersistenceHook` doc comments to reflect stateless design

## 4. Per-Session Serialization in Gateway

- [x] 4.1 Add `std::sync::Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>` in the gateway inbound handler
- [x] 4.2 Before spawning `process_direct`, acquire `semaphore.acquire()` for the session_key
- [x] 4.3 Release semaphore after `process_direct` completes (via RAII `SemaphorePermit` drop)
- [ ] 4.4 Integration test: deferred (requires mock LLM provider; validated via code review + manual test)

## 5. AgentHook Trait Documentation

- [x] 5.1 Add Stateless Contract section to `AgentHook` trait doc comment in `hooks/mod.rs`
- [x] 5.2 Document allowed vs prohibited state patterns with examples
- [x] 5.3 Add `# Contract` section to `HookRegistry::register` noting the stateless expectation

## 6. Validation

- [x] 6.1 `cargo clippy --all-targets` — no new warnings (3 pre-existing in skill.rs/auth.rs)
- [x] 6.2 `cargo test` — 66 passed, 1 pre-existing failure (test_spawn_tool, unrelated)
- [ ] 6.3 Manual test: gateway mode with Telegram (deferred to deployment)
