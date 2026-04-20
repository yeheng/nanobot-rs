# GenericAgent-Inspired Evolution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Integrate GenericAgent's core innovations — self-evolving SOPs, structured plan execution via tools, monitored subagent delegation, and proactive working memory — into gasket's Rust architecture without breaking existing APIs.

**Architecture:** Extract `SteppableExecutor` as a foundation primitive from `KernelExecutor`, then build four modules on top: SOP-aware wiki (`PageType::Sop`), tool-based planning (`create_plan`), channel-monitored subagents (`MonitoredSpawner`), and caller-layer checkpointing. The LLM decides when to plan; no hardcoded FSM, no routing layer.

**Tech Stack:** Rust 2021, tokio, sqlx (SQLite), Tantivy, serde_json

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `gasket/engine/src/kernel/executor.rs` | Modify | Extract `SteppableExecutor` + `StepResult`; `KernelExecutor` composes it internally |
| `gasket/engine/src/kernel/mod.rs` | Modify | Export `SteppableExecutor` and `StepResult` |
| `gasket/engine/src/wiki/page.rs` | Modify | Add `PageType::Sop` variant with `as_str()`, `FromStr`, `directory()` |
| `gasket/engine/src/wiki/mod.rs` | Modify | Update re-exports; add `PageType::Sop` tests |
| `gasket/engine/src/wiki/store.rs` | Modify | Add `"sops"` directory to `init_dirs()` |
| `gasket/engine/src/hooks/evolution.rs` | Modify | Add SOP extraction path; enhance prompt with "No Execution, No Memory" + `verified` flag |
| `gasket/engine/src/subagents/monitor.rs` | Create | `MonitoredSpawner`, `MonitoredRunner`, `ProgressUpdate`, `Intervention` |
| `gasket/engine/src/subagents/mod.rs` | Modify | Export new monitor types |
| `gasket/engine/src/session/compactor.rs` | Modify | Add `CheckpointConfig` and `checkpoint()` method |
| `gasket/storage/src/lib.rs` | Modify | Add `session_checkpoints` table + `save_checkpoint` / `load_checkpoint` methods |

---

## Task 1: Extract SteppableExecutor

**Files:**
- Modify: `gasket/engine/src/kernel/executor.rs`
- Modify: `gasket/engine/src/kernel/mod.rs`
- Test: `gasket/engine/src/wiki/mod.rs` (existing test suite — run full workspace)

**Context:** `KernelExecutor::run_loop()` currently contains an inline `for` loop that does: build request → send → stream response → check if final → handle tool calls. We extract the loop body into `SteppableExecutor::step()` so external callers (like `MonitoredRunner`) can drive execution turn-by-turn.

---

- [ ] **Step 1: Add `StepResult` struct and make `TokenLedger` public**

In `gasket/engine/src/kernel/executor.rs`, after the `ToolCallResult` struct (line 37), add:

```rust
/// Result of executing one LLM iteration
#[derive(Debug)]
pub struct StepResult {
    pub response: ChatResponse,
    pub tool_results: Vec<ToolCallResult>,
    pub should_continue: bool,
}
```

Make `TokenLedger` public (change `struct TokenLedger` to `pub struct TokenLedger` on line 256, and `fn new()` / `fn accumulate()` to `pub fn new()` / `pub fn accumulate()`).

Also make `ExecutionState` and its methods `pub` (lines 223-250), since `MonitoredRunner` will need to construct one.

---

- [ ] **Step 2: Create `SteppableExecutor` struct**

After `TokenLedger` impl (line 276), add:

```rust
// ─────────────────────────────────────────────────────────────────────────────
// SteppableExecutor
// ─────────────────────────────────────────────────────────────────────────────

/// Steppable executor — one LLM call + optional tool execution per step().
/// External callers drive the loop; KernelExecutor composes this internally.
pub struct SteppableExecutor {
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    config: KernelConfig,
    spawner: Option<Arc<dyn SubagentSpawner>>,
    token_tracker: Option<Arc<crate::token_tracker::TokenTracker>>,
}

impl SteppableExecutor {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        tools: Arc<ToolRegistry>,
        config: KernelConfig,
    ) -> Self {
        Self {
            provider,
            tools,
            config,
            spawner: None,
            token_tracker: None,
        }
    }

    pub fn with_spawner(mut self, spawner: Arc<dyn SubagentSpawner>) -> Self {
        self.spawner = Some(spawner);
        self
    }

    pub fn with_token_tracker(mut self, tracker: Arc<crate::token_tracker::TokenTracker>) -> Self {
        self.token_tracker = Some(tracker);
        self
    }

    /// Execute one iteration: LLM call → optional tool calls → return result.
    ///
    /// `messages` is mutated in place (assistant response + tool results appended).
    /// `ledger` accumulates token usage across steps.
    pub async fn step(
        &self,
        messages: &mut Vec<ChatMessage>,
        ledger: &mut TokenLedger,
        event_tx: Option<&mpsc::Sender<StreamEvent>>,
    ) -> Result<StepResult, KernelError> {
        let request_handler = RequestHandler::new(&self.provider, &self.tools, &self.config);
        let executor = ToolExecutor::new(&self.tools, self.config.max_tool_result_chars);

        let request = request_handler.build_chat_request(messages);
        let stream_result = request_handler
            .send_with_retry(request)
            .await
            .map_err(|e| KernelError::Provider(e.to_string()))?;

        let response = self
            .get_response(stream_result, event_tx, ledger)
            .await?;

        KernelExecutor::log_token_usage(ledger, 0); // iteration unknown at this layer
        KernelExecutor::log_response(&response, 0, &[]);

        let is_final = response.tool_calls.is_empty();

        if is_final {
            if let Some(ref content) = response.content {
                messages.push(ChatMessage::assistant(content));
            }
            return Ok(StepResult {
                response,
                tool_results: vec![],
                should_continue: false,
            });
        }

        // Handle tool calls — mutates messages, returns results for progress reporting
        let tool_results = self
            .handle_tool_calls(&response, &executor, messages, event_tx)
            .await;

        Ok(StepResult {
            response,
            tool_results,
            should_continue: true,
        })
    }

    async fn get_response(
        &self,
        stream_result: ChatStream,
        event_tx: Option<&mpsc::Sender<StreamEvent>>,
        ledger: &mut TokenLedger,
    ) -> Result<ChatResponse, KernelError> {
        let (mut event_stream, response_future) = stream::stream_events(stream_result);

        if let Some(tx) = event_tx {
            while let Some(event) = event_stream.next().await {
                if tx.send(event).await.is_err() {
                    break;
                }
            }
        } else {
            while event_stream.next().await.is_some() {}
        }

        let response = response_future
            .await
            .map_err(|e| KernelError::Provider(e.to_string()))?;

        if let Some(ref api_usage) = response.usage {
            let usage = gasket_types::TokenUsage::from_api_fields(
                api_usage.input_tokens,
                api_usage.output_tokens,
            );
            ledger.accumulate(&usage);
        }

        Ok(response)
    }

    async fn handle_tool_calls(
        &self,
        response: &ChatResponse,
        executor: &ToolExecutor<'_>,
        messages: &mut Vec<ChatMessage>,
        event_tx: Option<&mpsc::Sender<StreamEvent>>,
    ) -> Vec<ToolCallResult> {
        if response.tool_calls.is_empty() {
            if let Some(ref c) = response.content {
                messages.push(ChatMessage::assistant(c));
            }
            return vec![];
        }

        messages.push(ChatMessage::assistant_with_tools(
            response.content.clone(),
            response.tool_calls.clone(),
        ));

        let mut ctx = ToolContext::default();
        if let Some(ref spawner) = self.spawner {
            ctx = ctx.spawner(spawner.clone());
        }
        if let Some(ref tracker) = self.token_tracker {
            ctx = ctx.token_tracker(tracker.clone());
        }

        let futures: Vec<_> = response
            .tool_calls
            .iter()
            .enumerate()
            .map(|(idx, tc)| {
                let tool_call = tc.clone();
                let ctx = ctx.clone();
                let tx = event_tx.cloned();
                async move {
                    let tool_name = tool_call.function.name.clone();
                    let tool_args = tool_call.function.arguments.to_string();

                    if let Some(ref sender) = tx {
                        let _ = sender
                            .send(StreamEvent::tool_start(&tool_name, Some(tool_args)))
                            .await;
                    }

                    let start = std::time::Instant::now();
                    let result = executor.execute_one(&tool_call, &ctx).await;
                    let duration = start.elapsed();

                    debug!(
                        "[Steppable] Tool {} -> done ({}ms)",
                        tool_name,
                        duration.as_millis()
                    );

                    if let Some(ref sender) = tx {
                        let _ = sender
                            .send(StreamEvent::tool_end(&tool_name, Some(result.output.clone())))
                            .await;
                    }

                    (idx, tool_call.id, tool_name, result.output)
                }
            })
            .collect();

        let mut results = futures_util::future::join_all(futures).await;
        results.sort_by_key(|(idx, _, _, _)| *idx);

        let mut tool_results = Vec::new();
        for (_, tool_call_id, tool_name, output) in results {
            messages.push(ChatMessage::tool_result(
                tool_call_id.clone(),
                tool_name.clone(),
                output.clone(),
            ));
            tool_results.push(ToolCallResult {
                tool_call_id,
                tool_name,
                output,
            });
        }
        tool_results
    }
}
```

**Note:** The `handle_tool_calls` method above is a copy of `KernelExecutor::handle_tool_calls` with `&mut Vec<ChatMessage>` instead of `&mut ExecutionState`. After this refactor, `KernelExecutor` should delegate to `SteppableExecutor` and the duplicate code in `KernelExecutor::handle_tool_calls` should be removed.

---

- [ ] **Step 3: Refactor `KernelExecutor` to compose `SteppableExecutor`**

In `KernelExecutor`:

1. Change `run_loop()` to use `SteppableExecutor` internally:

```rust
async fn run_loop(
    &self,
    state: &mut ExecutionState,
    ledger: &mut TokenLedger,
    event_tx: Option<&mpsc::Sender<StreamEvent>>,
    options: &ExecutorOptions<'_>,
) -> Result<ExecutionResult, KernelError> {
    let steppable = SteppableExecutor::new(
        self.provider.clone(),
        self.tools.clone(),
        self.config.clone(),
    )
    .with_spawner_opt(self.spawner.clone())
    .with_token_tracker_opt(self.token_tracker.clone());

    for iteration in 1..=self.config.max_iterations {
        debug!("[Kernel] iteration {}", iteration);

        let result = steppable
            .step(&mut state.messages, ledger, event_tx)
            .await?;

        if !result.should_continue {
            let content = result.response.content.unwrap_or_default();
            let reasoning = result.response.reasoning_content;
            return Ok(state.to_result(content, reasoning, ledger));
        }
    }

    Err(KernelError::MaxIterations(self.config.max_iterations))
}
```

2. Remove the old `handle_tool_calls`, `get_response`, `log_token_usage`, `log_response`, `check_final_response` methods from `KernelExecutor` — they move to `SteppableExecutor` or become module-level helpers.

Wait — `log_token_usage` and `log_response` are called from `run_loop` with iteration number, but `step()` doesn't know the iteration. We can either:
- Keep them in `KernelExecutor::run_loop` and call them after `step()`
- Or move them to `SteppableExecutor` and pass iteration as a parameter

The cleaner approach: keep `log_token_usage` and `log_response` as module-level `pub(crate)` functions, call them from `KernelExecutor::run_loop` after each `step()`.

```rust
// In run_loop, after step():
KernelExecutor::log_token_usage(ledger, iteration);
KernelExecutor::log_response(&result.response, iteration, options.vault_values);
```

So `SteppableExecutor::step()` should NOT call log_token_usage/log_response — those stay in `KernelExecutor`.

Let me revise Step 2's `step()` method: remove the `log_token_usage` and `log_response` calls from inside `step()`.

Also, `KernelExecutor::check_final_response` should stay as a method since `run_loop` needs it to extract the final result. Actually no — after `step()` returns with `should_continue=false`, we already know it's final. The content is in `result.response.content`. So `check_final_response` is no longer needed.

3. Remove `handle_tool_calls`, `get_response`, and `check_final_response` from `KernelExecutor` impl. Keep `log_token_usage` and `log_response` as `pub(crate)` methods.

---

- [ ] **Step 4: Update `gasket/engine/src/kernel/mod.rs` exports**

Add to the `pub use executor::{...}` line:

```rust
pub use executor::{ExecutionResult, ExecutorOptions, KernelExecutor, StepResult, SteppableExecutor, ToolExecutor};
```

---

- [ ] **Step 5: Run existing tests to verify zero breakage**

Run:
```bash
cargo test --workspace
```

Expected: All existing tests pass. If any fail, the refactor broke something.

---

- [ ] **Step 6: Commit**

```bash
git add gasket/engine/src/kernel/executor.rs gasket/engine/src/kernel/mod.rs
git commit -m "feat(kernel): extract SteppableExecutor from KernelExecutor

- Add StepResult with response + should_continue
- Make TokenLedger and ExecutionState public
- SteppableExecutor::step() does one LLM iteration
- KernelExecutor composes SteppableExecutor in run_loop
- Zero API change for existing callers

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 2: Extend Wiki for SOP

**Files:**
- Modify: `gasket/engine/src/wiki/page.rs`
- Modify: `gasket/engine/src/wiki/store.rs`
- Modify: `gasket/engine/src/wiki/mod.rs`
- Test: `gasket/engine/src/wiki/mod.rs` (existing inline tests)

---

- [ ] **Step 1: Add `PageType::Sop` variant**

In `gasket/engine/src/wiki/page.rs`, change the enum (lines 5-11):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PageType {
    Entity,
    Topic,
    Source,
    Sop,
}
```

---

- [ ] **Step 2: Update `as_str()` and add `directory()`**

Replace the `impl PageType` block (lines 13-21):

```rust
impl PageType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::Topic => "topic",
            Self::Source => "source",
            Self::Sop => "sop",
        }
    }

    pub fn directory(&self) -> &'static str {
        match self {
            Self::Entity => "entities",
            Self::Topic => "topics",
            Self::Source => "sources",
            Self::Sop => "sops",
        }
    }
}
```

---

- [ ] **Step 3: Update `FromStr`**

Replace the `FromStr` impl (lines 23-34):

```rust
impl std::str::FromStr for PageType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "entity" => Ok(Self::Entity),
            "topic" => Ok(Self::Topic),
            "source" => Ok(Self::Source),
            "sop" => Ok(Self::Sop),
            _ => Err(()),
        }
    }
}
```

---

- [ ] **Step 4: Add "sops" to `init_dirs()`**

In `gasket/engine/src/wiki/store.rs`, update `init_dirs()` (line 24-35):

```rust
pub async fn init_dirs(&self) -> Result<()> {
    for dir in &[
        "entities/people",
        "entities/projects",
        "entities/concepts",
        "topics",
        "sources",
        "sops",
    ] {
        fs::create_dir_all(self.wiki_root.join(dir)).await?;
    }
    Ok(())
}
```

---

- [ ] **Step 5: Add tests**

In `gasket/engine/src/wiki/mod.rs`, add to the test module (after line 98):

```rust
    #[test]
    fn test_page_type_sop() {
        assert_eq!("sop".parse(), Ok(PageType::Sop));
        assert_eq!(PageType::Sop.as_str(), "sop");
        assert_eq!(PageType::Sop.directory(), "sops");
    }

    #[test]
    fn test_sop_page_roundtrip() {
        let page = WikiPage::new(
            "sops/docker-build".to_string(),
            "Docker Build SOP".to_string(),
            PageType::Sop,
            "1. Check Dockerfile exists\n2. Run docker build".to_string(),
        );
        let md = page.to_markdown();
        let parsed = WikiPage::from_markdown("sops/docker-build".to_string(), &md).unwrap();
        assert_eq!(parsed.page_type, PageType::Sop);
        assert_eq!(parsed.title, "Docker Build SOP");
    }
```

---

- [ ] **Step 6: Run tests**

```bash
cargo test --package gasket-engine wiki::
```

Expected: All 3 new tests pass, existing tests still pass.

---

- [ ] **Step 7: Commit**

```bash
git add gasket/engine/src/wiki/page.rs gasket/engine/src/wiki/store.rs gasket/engine/src/wiki/mod.rs
git commit -m "feat(wiki): add PageType::Sop for SOP knowledge pages

- Add Sop variant with as_str(), directory(), FromStr
- Add sops/ directory to init_dirs()
- Add roundtrip tests

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 3: Enhance EvolutionHook for SOP Extraction

**Files:**
- Modify: `gasket/engine/src/hooks/evolution.rs`
- Test: `gasket/engine/src/hooks/evolution.rs` (add inline test module at bottom)

---

- [ ] **Step 1: Add `verified` field to `EvolutionMemory`**

In `gasket/engine/src/hooks/evolution.rs`, change the struct (lines 23-31):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvolutionMemory {
    title: String,
    #[serde(rename = "type")]
    memory_type: String,
    scenario: String,
    content: String,
    tags: Option<Vec<String>>,
    verified: bool,
    confidence: f32,
}
```

---

- [ ] **Step 2: Update extraction prompt**

Replace the prompt (lines 203-214):

```rust
        let user_prompt = format!(
            "You are a memory extraction sub-system.\n\
             Analyze the following conversation transcript and extract ONLY NEW, PERSISTENT facts, preferences, or actionable skills.\n\n\
             CRITICAL RULES:\n\
             1. DO NOT extract transient context (e.g., 'User said hello').\n\
             2. DO NOT extract information that is likely already known.\n\
             3. Focus on concrete nouns: names, explicit architectural choices, strict preferences.\n\
             4. 'No Execution, No Memory' — only include facts/skills confirmed by successful tool calls.\n\
             5. Classify each item:\n\
                - type: 'note' (factual) or 'skill' (procedural)\n\
                - scenario: 'profile' (user pref), 'knowledge' (env fact), 'procedure' (task skill)\n\
                - verified: true if backed by successful tool result\n\
                - confidence: 0.0-1.0 based on verification strength\n\
             If nothing NEW and VALUABLE is found, return an empty array [].\n\n\
             Output strict JSON array: [{{\"title\": string, \"type\": \"note\"|\"skill\", \"scenario\": \"profile\"|\"knowledge\"|\"procedure\", \"content\": string, \"tags\": [string], \"verified\": bool, \"confidence\": float}}].\n\n{}",
            conversation
        );
```

---

- [ ] **Step 3: Add `persist_as_sop` method**

Add after the existing `run_parallel` impl block (after line 340):

```rust
impl EvolutionHook {
    async fn persist_as_sop(
        &self,
        mem: &EvolutionMemory,
        page_store: &PageStore,
    ) -> Result<(), AgentError> {
        let slug = slugify(&mem.title);
        let path = format!("sops/{}", slug);

        // Deduplication
        let existing = page_store.list(PageFilter::default()).await.map_err(|e| {
            AgentError::Other(format!("EvolutionHook: failed to list pages for dedup: {}", e))
        })?;

        let is_duplicate = existing
            .iter()
            .any(|p| slugify(&p.title) == slug || p.path.contains(&slug));

        if is_duplicate {
            debug!("EvolutionHook: SOP '{}' already exists. Skipping.", mem.title);
            return Ok(());
        }

        let mut page = WikiPage::new(
            path,
            mem.title.clone(),
            PageType::Sop,
            format_sop_content(mem),
        );

        let mut tags = mem.tags.clone().unwrap_or_default();
        tags.push("auto_learned".to_string());
        if mem.verified {
            tags.push("verified".to_string());
        }
        page.tags = tags;

        page_store.write(&page).await.map_err(|e| {
            AgentError::Other(format!("EvolutionHook: failed to write SOP page: {}", e))
        })?;

        info!("EvolutionHook: created SOP page '{}'", mem.title);
        Ok(())
    }
}

fn format_sop_content(mem: &EvolutionMemory) -> String {
    format!(
        "## Trigger Scenario\n- {}\n\n## Steps\n{}\n\n## Confidence\n{:.1}%",
        mem.scenario,
        mem.content,
        mem.confidence * 100.0
    )
}
```

---

- [ ] **Step 4: Route memory types in persistence loop**

Replace the persistence loop inside `run_parallel` (lines 262-328) with:

```rust
        for mem in memories {
            let page_store = match &self.page_store {
                Some(ps) => ps,
                None => {
                    warn!("EvolutionHook: PageStore not configured, skipping memory extraction");
                    continue;
                }
            };

            match mem.memory_type.as_str() {
                "skill" => {
                    if let Err(e) = self.persist_as_sop(&mem, page_store).await {
                        warn!("EvolutionHook: failed to persist SOP '{}': {}", mem.title, e);
                    }
                }
                _ => {
                    // Existing note/topic path
                    let path_prefix = match mem.scenario.as_str() {
                        "profile" => "entities/people",
                        _ => "topics",
                    };
                    let page_type = match mem.scenario.as_str() {
                        "profile" => PageType::Entity,
                        _ => PageType::Topic,
                    };

                    let slug = slugify(&mem.title);
                    let page_path = format!("{}/{}", path_prefix, slug);

                    // Deduplication
                    let existing = match page_store.list(PageFilter::default()).await {
                        Ok(pages) => pages,
                        Err(e) => {
                            warn!("EvolutionHook: failed to list pages for dedup: {}", e);
                            continue;
                        }
                    };
                    let is_dup = existing
                        .iter()
                        .any(|p| slugify(&p.title) == slug || p.path.contains(&slug));
                    if is_dup {
                        continue;
                    }

                    let mut tags = mem.tags.clone().unwrap_or_default();
                    tags.push("auto_learned".to_string());

                    let page = WikiPage::new(page_path, mem.title, page_type, mem.content);
                    let mut page = page;
                    page.tags = tags;

                    if let Err(e) = page_store.write(&page).await {
                        warn!("EvolutionHook: failed to create wiki page: {}", e);
                    }
                }
            }
        }
```

---

- [ ] **Step 5: Add inline tests for SOP extraction**

At the bottom of `gasket/engine/src/hooks/evolution.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_with_verified() {
        let json = r#"[{"title":"Docker Build","type":"skill","scenario":"procedure","content":"Run docker build","tags":["docker"],"verified":true,"confidence":0.95}]"#;
        let mems = EvolutionHook::extract_json(json).unwrap();
        assert_eq!(mems.len(), 1);
        assert_eq!(mems[0].memory_type, "skill");
        assert!(mems[0].verified);
        assert!((mems[0].confidence - 0.95).abs() < 0.01);
    }

    #[test]
    fn test_format_sop_content() {
        let mem = EvolutionMemory {
            title: "Test".to_string(),
            memory_type: "skill".to_string(),
            scenario: "procedure".to_string(),
            content: "1. Step one\n2. Step two".to_string(),
            tags: None,
            verified: true,
            confidence: 0.9,
        };
        let content = super::format_sop_content(&mem);
        assert!(content.contains("Trigger Scenario"));
        assert!(content.contains("Step one"));
        assert!(content.contains("90.0%"));
    }
}
```

---

- [ ] **Step 6: Run tests**

```bash
cargo test --package gasket-engine hooks::evolution::tests
```

Expected: New tests pass.

---

- [ ] **Step 7: Commit**

```bash
git add gasket/engine/src/hooks/evolution.rs
git commit -m "feat(hooks): enhance EvolutionHook with SOP extraction

- Add verified + confidence fields to EvolutionMemory
- Enhance prompt with 'No Execution, No Memory' axiom
- Add persist_as_sop() writing PageType::Sop pages
- Route 'skill' type to sops/, other types to existing paths
- Add tests for JSON extraction and SOP formatting

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 4: Implement MonitoredSpawner

**Files:**
- Create: `gasket/engine/src/subagents/monitor.rs`
- Modify: `gasket/engine/src/subagents/mod.rs`
- Test: `gasket/engine/src/subagents/monitor.rs` (inline test module)

---

- [ ] **Step 1: Create `gasket/engine/src/subagents/monitor.rs`**

```rust
//! Monitored subagent execution — real-time progress + intervention via channels.
//!
//! Replaces GenericAgent's file-IO protocol with type-safe Rust channels.
//! No SQLite fallback — if the subagent crashes, state is lost (KISS).

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::kernel::{ExecutionState, KernelConfig, StepResult, SteppableExecutor, TokenLedger};
use crate::session::config::AgentConfig;
use crate::session::config::AgentConfigExt;
use crate::tools::ToolRegistry;
use gasket_providers::{ChatMessage, LlmProvider};

use super::manager::TaskSpec;
use super::tracker::SubagentResult;

// ── Types ────────────────────────────────────────────────────────

/// Progress events emitted by a monitored subagent.
#[derive(Debug, Clone)]
pub enum ProgressUpdate {
    Thinking { turn: usize },
    ToolStart { name: String },
    ToolResult { name: String, output: String },
    TurnComplete { turn: usize, summary: String },
    Done { result: String },
    Error { message: String },
}

/// Intervention commands sent to a monitored subagent.
#[derive(Debug, Clone)]
pub enum Intervention {
    Abort,
    AddKeyInfo(String),
    AppendPrompt(String),
    ExtendTurns(u32),
}

/// Handle to a monitored subagent — includes progress stream and intervention channel.
pub struct MonitoredHandle {
    pub handle: JoinHandle<SubagentResult>,
    pub interventor: mpsc::Sender<Intervention>,
    pub progress: mpsc::Receiver<ProgressUpdate>,
}

// ── MonitoredSpawner ─────────────────────────────────────────────

/// Spawns subagents with real-time monitoring and intervention.
pub struct MonitoredSpawner;

impl MonitoredSpawner {
    pub fn spawn(
        provider: Arc<dyn LlmProvider>,
        tools: Arc<ToolRegistry>,
        spec: TaskSpec,
    ) -> Result<MonitoredHandle, anyhow::Error> {
        let (progress_tx, progress_rx) = mpsc::channel(64);
        let (interventor_tx, interventor_rx) = mpsc::channel(16);

        let config = AgentConfig {
            model: spec.model.clone().unwrap_or_else(|| provider.default_model().to_string()),
            max_iterations: 10,
            ..Default::default()
        };
        let kernel_config = config.to_kernel_config();

        let steppable = SteppableExecutor::new(provider, tools, kernel_config);

        let handle = tokio::spawn(async move {
            let mut runner = MonitoredRunner::new(spec, steppable, progress_tx, interventor_rx);
            match runner.run().await {
                Ok(result) => result,
                Err(e) => {
                    let _ = runner
                        .progress
                        .send(ProgressUpdate::Error {
                            message: e.to_string(),
                        })
                        .await;
                    SubagentResult {
                        id: runner.spec.id,
                        task: runner.spec.task,
                        response: crate::tools::SubagentResponse {
                            content: format!("Error: {}", e),
                            reasoning_content: None,
                            tools_used: vec![],
                            model: None,
                            token_usage: None,
                            cost: 0.0,
                        },
                        model: None,
                    }
                }
            }
        });

        Ok(MonitoredHandle {
            handle,
            interventor: interventor_tx,
            progress: progress_rx,
        })
    }
}

// ── MonitoredRunner ──────────────────────────────────────────────

struct MonitoredRunner {
    spec: TaskSpec,
    steppable: SteppableExecutor,
    messages: Vec<ChatMessage>,
    ledger: TokenLedger,
    progress: mpsc::Sender<ProgressUpdate>,
    intervention: mpsc::Receiver<Intervention>,
    max_turns: u32,
}

impl MonitoredRunner {
    fn new(
        spec: TaskSpec,
        steppable: SteppableExecutor,
        progress: mpsc::Sender<ProgressUpdate>,
        intervention: mpsc::Receiver<Intervention>,
    ) -> Self {
        let system = spec.system_prompt.clone().unwrap_or_default();
        let messages = if system.is_empty() {
            vec![ChatMessage::user(&spec.task)]
        } else {
            vec![ChatMessage::system(&system), ChatMessage::user(&spec.task)]
        };

        Self {
            spec,
            steppable,
            messages,
            ledger: TokenLedger::new(),
            progress,
            intervention,
            max_turns: 10,
        }
    }

    async fn run(&mut self) -> Result<SubagentResult, anyhow::Error> {
        for turn in 1..=self.max_turns {
            // Check for interventions (non-blocking)
            while let Ok(i) = self.intervention.try_recv() {
                match i {
                    Intervention::Abort => {
                        info!("[Monitored {}] Abort requested", self.spec.id);
                        let result = self.final_result();
                        let _ = self
                            .progress
                            .send(ProgressUpdate::Done {
                                result: result.response.content.clone(),
                            })
                            .await;
                        return Ok(result);
                    }
                    Intervention::AddKeyInfo(info) => {
                        self.messages.push(ChatMessage::system(format!(
                            "[Key Info] {}",
                            info
                        )));
                    }
                    Intervention::AppendPrompt(prompt) => {
                        self.messages.push(ChatMessage::user(prompt));
                    }
                    Intervention::ExtendTurns(n) => {
                        self.max_turns += n;
                    }
                }
            }

            let _ = self
                .progress
                .send(ProgressUpdate::Thinking { turn: turn as usize })
                .await;

            let result = self
                .steppable
                .step(&mut self.messages, &mut self.ledger, None)
                .await
                .map_err(|e| anyhow::anyhow!("Step failed: {}", e))?;

            if !result.tool_results.is_empty() {
                for tr in &result.tool_results {
                    let _ = self
                        .progress
                        .send(ProgressUpdate::ToolStart {
                            name: tr.tool_name.clone(),
                        })
                        .await;
                    let _ = self
                        .progress
                        .send(ProgressUpdate::ToolResult {
                            name: tr.tool_name.clone(),
                            output: tr.output.clone(),
                        })
                        .await;
                }
            }

            let summary = result
                .response
                .content
                .clone()
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            let _ = self
                .progress
                .send(ProgressUpdate::TurnComplete {
                    turn: turn as usize,
                    summary,
                })
                .await;

            if !result.should_continue {
                let final_result = self.final_result();
                let _ = self
                    .progress
                    .send(ProgressUpdate::Done {
                        result: final_result.response.content.clone(),
                    })
                    .await;
                return Ok(final_result);
            }
        }

        let final_result = self.final_result();
        let _ = self
            .progress
            .send(ProgressUpdate::Done {
                result: final_result.response.content.clone(),
            })
            .await;
        Ok(final_result)
    }

    fn final_result(&self) -> SubagentResult {
        let content = self
            .messages
            .last()
            .and_then(|m| m.content.clone())
            .unwrap_or_default();

        SubagentResult {
            id: self.spec.id.clone(),
            task: self.spec.task.clone(),
            response: crate::tools::SubagentResponse {
                content,
                reasoning_content: None,
                tools_used: vec![], // TODO: track from ledger or state
                model: None,
                token_usage: self.ledger.total_usage.clone(),
                cost: 0.0,
            },
            model: self.spec.model.clone(),
        }
    }
}
```

---

## Task 5: Implement Compactor Checkpoint

**Files:**
- Modify: `gasket/engine/src/session/compactor.rs`
- Modify: `gasket/storage/src/lib.rs`
- Test: `gasket/engine/src/session/compactor.rs` (inline test module)

**Context:** Add proactive working-memory snapshots every N turns. Unlike passive compaction (triggered by token threshold), checkpointing is proactive and turn-driven. It generates a structured summary of current task state and persists it to SQLite.

---

- [ ] **Step 1: Add `session_checkpoints` table to storage**

In `gasket/storage/src/lib.rs`, inside `init_db()` (after the `session_summaries` table creation, around line 364), add:

```rust
        // ── Session checkpoints ──
        // Proactive working-memory snapshots every N turns.
        // Called by MonitoredRunner between step() calls.

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS session_checkpoints (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                session_key TEXT NOT NULL,
                turn        INTEGER NOT NULL,
                summary     TEXT NOT NULL,
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(session_key, turn)
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_session_checkpoints_key_turn
             ON session_checkpoints(session_key, turn)",
        )
        .execute(&self.pool)
        .await?;
```

Then add two methods to `SqliteStore` (after `write_raw`, around line 680):

```rust
    /// Save a checkpoint summary for a session at a specific turn.
    pub async fn save_checkpoint(
        &self,
        session_key: &str,
        turn: i64,
        summary: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO session_checkpoints (session_key, turn, summary, created_at)
             VALUES ($1, $2, $3, datetime('now'))"
        )
        .bind(session_key)
        .bind(turn)
        .bind(summary)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Load the most recent checkpoint for a session before or at a given turn.
    pub async fn load_checkpoint(
        &self,
        session_key: &str,
        turn: i64,
    ) -> anyhow::Result<Option<(String, i64)>> {
        let row = sqlx::query_as::<(String, i64)>(
            "SELECT summary, turn FROM session_checkpoints
             WHERE session_key = $1 AND turn <= $2
             ORDER BY turn DESC
             LIMIT 1"
        )
        .bind(session_key)
        .bind(turn)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }
```

---

- [ ] **Step 2: Add `CheckpointConfig` struct**

In `gasket/engine/src/session/compactor.rs`, after the `UsageStats` struct (line 92), add:

```rust
/// Configuration for proactive checkpointing.
#[derive(Debug, Clone)]
pub struct CheckpointConfig {
    /// Trigger checkpoint every N turns (0 = disabled).
    pub interval_turns: usize,
    /// Prompt template for checkpoint generation.
    pub prompt: String,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            interval_turns: 7,
            prompt: r#"Summarize current task state for working memory.
Output ONLY in this format:

<key_info>
- Current goal: [one sentence]
- Completed: [list]
- Blocked on: [if any]
- Next step: [one sentence]
- Key facts learned: [list]
</key_info>

Be concise."#
            .into(),
        }
    }
}
```

---

- [ ] **Step 3: Add `checkpoint()` method to `ContextCompactor`**

Add a field to `ContextCompactor` (line 116-133):

```rust
pub struct ContextCompactor {
    provider: Arc<dyn LlmProvider>,
    event_store: Arc<EventStore>,
    sqlite_store: Arc<SqliteStore>,
    model: String,
    token_budget: usize,
    compaction_threshold: f32,
    summarization_prompt: String,
    is_compressing: Arc<AtomicBool>,
    checkpoint_config: Option<CheckpointConfig>,
}
```

Update `ContextCompactor::new()` to initialize `checkpoint_config: None`.

Add a builder method:

```rust
    /// Enable proactive checkpointing.
    pub fn with_checkpoint_config(mut self, config: CheckpointConfig) -> Self {
        self.checkpoint_config = Some(config);
        self
    }
```

Add the `checkpoint()` method (after `force_compact_and_wait`, around line 311):

```rust
    /// Generate a proactive checkpoint for the current session state.
    ///
    /// Called by the caller layer (e.g. MonitoredRunner) every N turns.
    /// Returns `Some(summary)` if a checkpoint was generated, `None` if skipped.
    pub async fn checkpoint(
        &self,
        session_key: &SessionKey,
        current_turn: usize,
        recent_events: &[SessionEvent],
    ) -> Result<Option<String>> {
        let config = match &self.checkpoint_config {
            Some(c) => c,
            None => return Ok(None),
        };

        if config.interval_turns == 0 || current_turn % config.interval_turns != 0 {
            return Ok(None);
        }

        let events_text = recent_events
            .iter()
            .map(|e| format!("{}: {}", e.event_type, e.content))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!(
            "{}\n\nRecent events:\n{}",
            config.prompt, events_text
        );

        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage::system("You are a state summarizer."),
                ChatMessage::user(prompt),
            ],
            tools: None,
            temperature: Some(0.2),
            max_tokens: Some(512),
            thinking: None,
        };

        let response = self.provider.chat(request).await?;
        let summary = response.content.unwrap_or_default().trim().to_string();

        if summary.is_empty() {
            warn!("Checkpoint generated empty summary for {}", session_key);
            return Ok(None);
        }

        self.sqlite_store
            .save_checkpoint(&session_key.to_string(), current_turn as i64, &summary)
            .await?;

        info!(
            "Checkpoint saved for {} at turn {} ({} chars)",
            session_key,
            current_turn,
            summary.len()
        );

        Ok(Some(summary))
    }
```

---

- [ ] **Step 4: Add inline tests**

At the bottom of `gasket/engine/src/session/compactor.rs`, add:

```rust
#[cfg(test)]
mod checkpoint_tests {
    use super::*;

    #[test]
    fn test_checkpoint_config_default() {
        let config = CheckpointConfig::default();
        assert_eq!(config.interval_turns, 7);
        assert!(config.prompt.contains("Current goal"));
    }
}
```

---

- [ ] **Step 5: Run tests**

```bash
cargo test --package gasket-engine session::compactor::checkpoint_tests
cargo test --package gasket-storage
```

Expected: New tests pass, existing tests still pass.

---

- [ ] **Step 6: Commit**

```bash
git add gasket/engine/src/session/compactor.rs gasket/storage/src/lib.rs
git commit -m "feat(compactor): add proactive checkpointing

- Add session_checkpoints table with unique (session_key, turn) constraint
- Add CheckpointConfig with interval_turns + prompt template
- Add ContextCompactor::checkpoint() for caller-layer state snapshots
- Add save_checkpoint / load_checkpoint to SqliteStore
- Add tests for CheckpointConfig defaults

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## End of Plan
