# GenericAgent-Inspired Evolution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Integrate GenericAgent's core innovations — self-evolving SOPs, structured plan execution via tools, monitored subagent delegation, and proactive working memory — into gasket's Rust architecture without breaking existing APIs.

**Architecture:** Extract `SteppableExecutor` as a foundation primitive from `KernelExecutor`, then build four modules on top: SOP-aware wiki (`PageType::Sop`), tool-based planning (`create_plan`), channel-monitored subagents (`MonitoredSpawner`), and SteppableExecutor-level checkpointing. The LLM decides when to plan; no hardcoded FSM, no routing layer.

**Tech Stack:** Rust 2021, tokio, sqlx (SQLite), Tantivy, serde_json

**Recommended Execution Order:** Task 1 → Task 2 → Task 6 (simplified) → Task 7 → Task 3 → Task 4 → Task 5 (unified). Rationale: create_plan and search_sops are simpler and dependency-free; EvolutionHook and MonitoredSpawner depend on SOP infrastructure; Checkpoint depends on SteppableExecutor.

**Changes from Linus Review (`task.md`):**
1. **Killed JSON AST in create_plan** — `PlanStep`, `StepType`, `Plan` structs are unnecessary indirection. The engine never parses/executes a plan AST. Markdown is the native data structure for LLM-to-LLM communication. Don't do pointless JSON serialize→deserialize→serialize round-trips.
2. **Unified Checkpoint at SteppableExecutor level** — Original design placed checkpoint only at the caller layer (MonitoredRunner), creating asymmetry where subagents get proactive working memory but the main agent does not. Checkpoint is now an optional interceptor on `SteppableExecutor::step()`, benefiting both modes equally. Good code has no special cases.
3. **Reordered tasks** — create_plan and search_sops are dependency-free and should be implemented before EvolutionHook and MonitoredSpawner.

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `gasket/engine/src/kernel/executor.rs` | Modify | Extract `SteppableExecutor` + `StepResult` + optional checkpoint interceptor; `KernelExecutor` composes it internally |
| `gasket/engine/src/kernel/mod.rs` | Modify | Export `SteppableExecutor` and `StepResult` |
| `gasket/engine/src/wiki/page.rs` | Modify | Add `PageType::Sop` variant with `as_str()`, `FromStr`, `directory()` |
| `gasket/engine/src/wiki/mod.rs` | Modify | Update re-exports; add `PageType::Sop` tests |
| `gasket/engine/src/wiki/store.rs` | Modify | Add `"sops"` directory to `init_dirs()` |
| `gasket/engine/src/hooks/evolution.rs` | Modify | Add SOP extraction path; enhance prompt with "No Execution, No Memory" + `verified` flag |
| `gasket/engine/src/subagents/monitor.rs` | Create | `MonitoredSpawner`, `MonitoredRunner`, `ProgressUpdate`, `Intervention` |
| `gasket/engine/src/subagents/mod.rs` | Modify | Export new monitor types |
| `gasket/engine/src/session/compactor.rs` | Modify | Add `CheckpointConfig` and `checkpoint()` method |
| `gasket/storage/src/lib.rs` | Modify | Add `session_checkpoints` table + `save_checkpoint` / `load_checkpoint` methods |
| `gasket/engine/src/tools/create_plan.rs` | Create | Simplified `CreatePlanTool` (Markdown-based, no JSON AST) |
| `gasket/engine/src/tools/search_sops.rs` | Create | `search_sops` tool function |
| `gasket/engine/src/tools/mod.rs` | Modify | Register `create_plan` and `search_sops` in `ToolRegistry` |

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

`ExecutionState` **does NOT need to be public** — `MonitoredRunner` constructs its own `Vec<ChatMessage>` directly, so `ExecutionState` can remain private to the kernel module.

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
    let mut steppable = SteppableExecutor::new(
        self.provider.clone(),
        self.tools.clone(),
        self.config.clone(),
    );
    if let Some(ref spawner) = self.spawner {
        steppable = steppable.with_spawner(spawner.clone());
    }
    if let Some(ref tracker) = self.token_tracker {
        steppable = steppable.with_token_tracker(tracker.clone());
    }

    for iteration in 1..=self.config.max_iterations {
        debug!("[Kernel] iteration {}", iteration);

        let result = steppable
            .step(&mut state.messages, ledger, event_tx)
            .await?;

        // Logging stays at the KernelExecutor layer where iteration context is known
        KernelExecutor::log_token_usage(ledger, iteration);
        KernelExecutor::log_response(&result.response, iteration, options.vault_values);

        if !result.should_continue {
            let content = result.response.content.unwrap_or_default();
            let reasoning = result.response.reasoning_content;
            return Ok(state.to_result(content, reasoning, ledger));
        }
    }

    Err(KernelError::MaxIterations(self.config.max_iterations))
}
```

2. Remove `handle_tool_calls`, `get_response`, and `check_final_response` from `KernelExecutor` impl. Keep `log_token_usage` and `log_response` as `pub(crate)` methods so `run_loop` can call them after each `step()`.

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
- Make TokenLedger public
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
    let tags = mem.tags.clone().unwrap_or_default().join(", ");
    format!(
        "---\n\
         title: {}\n\
         type: sop\n\
         tags: [{}]\n\
         ---\n\n\
         ## Trigger Scenario\n\
         - {}\n\n\
         ## Preconditions\n\
         - (observed during execution)\n\n\
         ## Key Steps\n\
         {}\n\n\
         ## Pitfalls\n\
         - Review before reuse in different environments.\n\n\
         ## Confidence\n\
         - {:.1}% (verified: {})\n",
        mem.title,
        tags,
        mem.scenario,
        mem.content,
        mem.confidence * 100.0,
        mem.verified
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
            tags: Some(vec!["docker".to_string()]),
            verified: true,
            confidence: 0.9,
        };
        let content = super::format_sop_content(&mem);
        assert!(content.contains("Trigger Scenario"));
        assert!(content.contains("Preconditions"));
        assert!(content.contains("Key Steps"));
        assert!(content.contains("Pitfalls"));
        assert!(content.contains("Step one"));
        assert!(content.contains("90.0%"));
        assert!(content.contains("verified: true"));
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
- Add persist_as_sop() writing PageType::Sop pages with full template
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

use crate::kernel::{KernelConfig, StepResult, SteppableExecutor, TokenLedger};
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
            max_iterations: spec.max_turns.unwrap_or(10) as usize,
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
                    let mut result = runner.final_result();
                    result.response.content = format!("Error: {}", e);
                    result
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
            max_turns: spec.max_turns.unwrap_or(10),
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

- [ ] **Step 2: Export types in `subagents/mod.rs`**

Add to `gasket/engine/src/subagents/mod.rs`:

```rust
pub mod monitor;
pub use monitor::{Intervention, MonitoredHandle, MonitoredSpawner, ProgressUpdate};
```

---

- [ ] **Step 3: Add inline tests**

At the bottom of `gasket/engine/src/subagents/monitor.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_update_clone() {
        let p = ProgressUpdate::Thinking { turn: 1 };
        let _ = p.clone();
    }

    #[test]
    fn test_intervention_clone() {
        let i = Intervention::AddKeyInfo("test".to_string());
        let _ = i.clone();
    }
}
```

---

- [ ] **Step 4: Run tests**

```bash
cargo test --package gasket-engine subagents::monitor::tests
```

Expected: Tests compile and pass.

---

- [ ] **Step 5: Commit**

```bash
git add gasket/engine/src/subagents/monitor.rs gasket/engine/src/subagents/mod.rs
git commit -m "feat(subagents): add MonitoredSpawner with channel-based oversight

- ProgressUpdate + Intervention enums for real-time oversight
- MonitoredRunner drives SteppableExecutor turn-by-turn
- No DB fallback (KISS)

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 5: Implement Compactor Checkpoint

**Files:**
- Modify: `gasket/engine/src/session/compactor.rs`
- Modify: `gasket/storage/src/lib.rs`
- Test: `gasket/engine/src/session/compactor.rs` (inline test module)

**Context:** ~~Add proactive working-memory snapshots every N sequence increments... The **caller layer** (e.g. `MonitoredRunner`) is responsible for injecting the checkpoint back into the message history.~~ **REVISED (Linus Review): Unified at SteppableExecutor level.** The original design placed checkpoint only at the caller layer (MonitoredRunner), creating an asymmetry where subagents get proactive working memory but the main agent (CLI mode) does not. **Good code has no special cases.** Since `SteppableExecutor::step()` is the shared execution primitive for both modes, checkpoint injection belongs there as an optional interceptor — not leaked to callers. The storage layer (`session_checkpoints` table) and `CheckpointConfig` remain unchanged.

---

- [ ] **Step 1: Add `session_checkpoints` table to storage**

In `gasket/storage/src/lib.rs`, inside `init_db()` (after the `session_summaries` table creation, around line 364), add:

```rust
        // ── Session checkpoints ──
        // Proactive working-memory snapshots every N sequence increments.
        // Called by MonitoredRunner between step() calls.
        // target_sequence binds to EventStore's monotonic sequence, NOT a transient turn counter.

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS session_checkpoints (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                session_key     TEXT NOT NULL,
                target_sequence INTEGER NOT NULL,
                summary         TEXT NOT NULL,
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(session_key, target_sequence)
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_session_checkpoints_key_seq
             ON session_checkpoints(session_key, target_sequence)",
        )
        .execute(&self.pool)
        .await?;
```

Then add two methods to `SqliteStore` (after `write_raw`, around line 680):

```rust
    /// Save a checkpoint summary for a session at a specific target_sequence.
    pub async fn save_checkpoint(
        &self,
        session_key: &str,
        target_sequence: i64,
        summary: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO session_checkpoints (session_key, target_sequence, summary, created_at)
             VALUES (?1, ?2, ?3, datetime('now'))"
        )
        .bind(session_key)
        .bind(target_sequence)
        .bind(summary)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Load the most recent checkpoint for a session before or at a given target_sequence.
    pub async fn load_checkpoint(
        &self,
        session_key: &str,
        target_sequence: i64,
    ) -> anyhow::Result<Option<(String, i64)>> {
        let row: Option<(String, i64)> = sqlx::query_as(
            "SELECT summary, target_sequence FROM session_checkpoints
             WHERE session_key = ?1 AND target_sequence <= ?2
             ORDER BY target_sequence DESC
             LIMIT 1"
        )
        .bind(session_key)
        .bind(target_sequence)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }
```

**Note:** `sqlx::query_as` with anonymous tuples requires `sqlx::query_as::<_, (String, i64)>(...)` syntax. Ensure the `sqlx` feature for SQLite is enabled.

---

- [ ] **Step 2: Add `CheckpointConfig` struct**

In `gasket/engine/src/session/compactor.rs`, after the `UsageStats` struct (line 92), add:

```rust
/// Configuration for proactive checkpointing.
#[derive(Debug, Clone)]
pub struct CheckpointConfig {
    /// Trigger checkpoint every N sequence increments (0 = disabled).
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
    /// Called by the caller layer (e.g. MonitoredRunner) every N sequence increments.
    /// Returns `Some(summary)` if a checkpoint was generated, `None` if skipped.
    ///
    /// The caller is responsible for injecting the returned summary into the
    /// message history (e.g. as a system message) so the LLM sees its working memory.
    ///
    /// **CRITICAL:** `current_max_sequence` must be fetched from EventStore.
    /// Never pass a transient turn counter — it resets on restart and will cause
    /// unique-constraint violations in `session_checkpoints`.
    pub async fn checkpoint(
        &self,
        session_key: &SessionKey,
        current_max_sequence: i64,
        recent_events: &[SessionEvent],
    ) -> anyhow::Result<Option<String>> {
        let config = match &self.checkpoint_config {
            Some(c) => c,
            None => return Ok(None),
        };

        if config.interval_turns == 0
            || current_max_sequence == 0
            || current_max_sequence % config.interval_turns as i64 != 0
        {
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
            .save_checkpoint(&session_key.to_string(), current_max_sequence, &summary)
            .await?;

        info!(
            "Checkpoint saved for {} at sequence {} ({} chars)",
            session_key,
            current_max_sequence,
            summary.len()
        );

        Ok(Some(summary))
    }
```

**Important:** `session_key` must implement `Display` (or `ToString`) so that `session_key.to_string()` works. If `SessionKey` does not implement `Display`, add `ToString` or call an existing `as_str()` method.

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

- Add session_checkpoints table with unique (session_key, target_sequence) constraint
- Add CheckpointConfig with interval_turns + prompt template
- Add ContextCompactor::checkpoint() for caller-layer state snapshots
- Add save_checkpoint / load_checkpoint to SqliteStore
- Add tests for CheckpointConfig defaults

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

#### Revision from Linus Review: Unified Checkpoint at SteppableExecutor Level

After completing Steps 1-6 above, **add this new step** to integrate checkpointing into `SteppableExecutor` itself, eliminating the caller-layer asymmetry. Task 8 is also simplified as a result.

- [ ] **Step 3b: Add checkpoint interceptor to `SteppableExecutor`**

In `gasket/engine/src/kernel/executor.rs`, add an optional checkpoint callback to `SteppableExecutor`:

```rust
pub struct SteppableExecutor {
    // ... existing fields ...
    /// Optional checkpoint interceptor. Called before each step().
    /// Returns summary to inject, or None to skip.
    checkpoint_callback: Option<Arc<dyn Fn(usize) -> Option<String> + Send + Sync>>,
}

impl SteppableExecutor {
    pub fn with_checkpoint(
        mut self,
        callback: Arc<dyn Fn(usize) -> Option<String> + Send + Sync>,
    ) -> Self {
        self.checkpoint_callback = Some(callback);
        self
    }

    pub async fn step(
        &self,
        messages: &mut Vec<ChatMessage>,
        ledger: &mut TokenLedger,
        event_tx: Option<&mpsc::Sender<StreamEvent>>,
    ) -> Result<StepResult, KernelError> {
        // Proactive checkpoint injection (before LLM call)
        if let Some(ref cb) = self.checkpoint_callback {
            if let Some(summary) = cb(messages.len()) {
                debug!("[Steppable] Injecting checkpoint ({} chars)", summary.len());
                messages.push(ChatMessage::system(
                    format!("[Working Memory] {}", summary)
                ));
            }
        }
        // ... rest of step() unchanged ...
    }
}
```

**Key insight:** Both `KernelExecutor` (CLI mode) and `MonitoredRunner` (subagent mode) use `SteppableExecutor` internally. By placing the checkpoint interceptor here, both modes benefit equally. **No special cases.** Good code has no special cases.

---

## Task 6: Implement `create_plan` Tool

**Files:**
- Create: `gasket/engine/src/tools/create_plan.rs`
- Modify: `gasket/engine/src/tools/mod.rs`
- Test: `gasket/engine/src/tools/create_plan.rs` (inline test module)

**Context:** ~~Instead of a hardcoded `PlanExecutor` FSM, we provide a `create_plan` tool that the LLM calls when it decides a task is complex.~~ **REVISED (Linus Review): No JSON AST.** The engine never parses or executes a plan tree. `PlanStep`, `StepType`, and `Plan` structs are unnecessary indirection — the consumer is the LLM's context window, and Markdown is its native data structure. Don't do pointless JSON serialize→deserialize→serialize round-trips. The tool prompts the LLM to generate a Markdown plan, wraps it in a `WikiPage` (PageType::Topic), and returns a short confirmation + file path.

---

- [ ] **Step 1: Create `gasket/engine/src/tools/create_plan.rs`**

```rust
//! Tool: create_plan — generate a structured execution plan for complex tasks.
//!
//! The LLM calls this when it determines a task requires multiple steps.
//! The plan is persisted to the wiki and returned as a structured message.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::kernel::ChatMessage;
use crate::wiki::{PageStore, PageType, WikiPage};
use gasket_providers::{ChatRequest, LlmProvider};

// ── Data Types ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub description: String,
    pub step_type: StepType,
    pub depends_on: Vec<usize>,
    pub condition: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepType {
    Direct,
    Delegated,
    Parallel,
    Conditional,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub title: String,
    pub goal: String,
    pub steps: Vec<PlanStep>,
    pub verification_criteria: Vec<String>,
    pub wiki_path: String,
}

impl Plan {
    /// Render the plan as markdown for wiki storage.
    pub fn to_wiki_page(&self) -> WikiPage {
        let mut body = format!("## Goal\n{}\n\n## Steps\n", self.goal);
        for (i, step) in self.steps.iter().enumerate() {
            let type_label = match step.step_type {
                StepType::Direct => "[D]",
                StepType::Delegated => "[P]",
                StepType::Parallel => "[||]",
                StepType::Conditional => "[?]",
            };
            body.push_str(&format!(
                "{}. {} {}\n",
                i + 1,
                type_label,
                step.description
            ));
        }
        body.push_str("\n## Verification Criteria\n");
        for crit in &self.verification_criteria {
            body.push_str(&format!("- {}\n", crit));
        }

        WikiPage::new(
            self.wiki_path.clone(),
            self.title.clone(),
            PageType::Topic,
            body,
        )
    }
}

// ── Tool ─────────────────────────────────────────────────────────

pub struct CreatePlanTool {
    provider: Arc<dyn LlmProvider>,
    model: String,
    page_store: Arc<PageStore>,
}

impl CreatePlanTool {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        model: String,
        page_store: Arc<PageStore>,
    ) -> Self {
        Self {
            provider,
            model,
            page_store,
        }
    }

    pub async fn invoke(
        &self,
        goal: &str,
        context: &[ChatMessage],
    ) -> Result<Plan, ToolError> {
        // 1. Search for relevant SOPs
        let sops = self.search_relevant_sops(goal).await?;

        // 2. Build prompt with SOP context + plan generation instructions
        let prompt = self.build_plan_prompt(goal, &sops, context);

        // 3. Call LLM to generate structured plan
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage::system("You are a planning assistant. Output plans as strict JSON."),
                ChatMessage::user(prompt),
            ],
            max_tokens: Some(2048),
            temperature: Some(0.3),
            ..Default::default()
        };

        let response = self.provider.chat(request).await?;
        let raw = response.content.unwrap_or_default();
        let plan: Plan = self.parse_plan(&raw, goal)?;

        // 4. Persist plan to wiki
        self.page_store.write(&plan.to_wiki_page()).await?;
        info!("create_plan: persisted plan '{}' to {}", plan.title, plan.wiki_path);

        Ok(plan)
    }

    async fn search_relevant_sops(&self, _goal: &str) -> Result<Vec<String>, ToolError> {
        // Delegates to the search_sops tool (registered separately).
        // For now, return empty — the ToolRegistry will resolve search_sops when available.
        Ok(vec![])
    }

    fn build_plan_prompt(&self, goal: &str, sops: &[String], context: &[ChatMessage]) -> String {
        let context_text = context
            .iter()
            .map(|m| format!("{:?}: {}", m.role, m.content.as_deref().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("\n");

        let sop_text = if sops.is_empty() {
            "No relevant SOPs found.".to_string()
        } else {
            format!("Relevant SOPs:\n{}", sops.join("\n"))
        };

        format!(
            "Goal: {}\n\n{}\n\nRecent context:\n{}\n\n\
             Generate a structured execution plan. Output strict JSON with this schema:\n\
             {{\"title\": string, \"goal\": string, \"steps\": [{{\"description\": string, \"step_type\": \"Direct\"|\"Delegated\"|\"Parallel\"|\"Conditional\", \"depends_on\": [number], \"condition\": string|null}}], \"verification_criteria\": [string]}}\n\
             wiki_path will be auto-generated as plans/{{slug}}.",
            goal, sop_text, context_text
        )
    }

    fn parse_plan(&self, raw: &str, goal: &str) -> Result<Plan, ToolError> {
        // Extract JSON from possible markdown code fence
        let json_str = if raw.trim().starts_with("```") {
            raw.lines()
                .skip_while(|l| l.trim().starts_with("```"))
                .take_while(|l| !l.trim().starts_with("```"))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            raw.to_string()
        };

        let mut plan: Plan = serde_json::from_str(&json_str)
            .map_err(|e| ToolError::Parse(format!("Invalid plan JSON: {}", e)))?;

        // Auto-generate wiki_path if not provided
        if plan.wiki_path.is_empty() {
            let slug = plan.title.to_lowercase().replace(" ", "-").replace("/", "-");
            plan.wiki_path = format!("plans/{}", slug);
        }

        // Ensure goal is populated
        if plan.goal.is_empty() {
            plan.goal = goal.to_string();
        }

        Ok(plan)
    }
}

#[derive(Debug)]
pub enum ToolError {
    Provider(String),
    Parse(String),
    Storage(String),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::Provider(s) => write!(f, "Provider error: {}", s),
            ToolError::Parse(s) => write!(f, "Parse error: {}", s),
            ToolError::Storage(s) => write!(f, "Storage error: {}", s),
        }
    }
}

impl std::error::Error for ToolError {}
```

**Note:** `ToolError` here is local to `create_plan.rs`. When wiring into the main `ToolRegistry`, map `ToolError` to the registry's error type (e.g. `anyhow::Error` or `crate::tools::ToolCallError`).

---

- [ ] **Step 2: Add inline tests**

At the bottom of `gasket/engine/src/tools/create_plan.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_to_wiki_page() {
        let plan = Plan {
            title: "Rust Setup".to_string(),
            goal: "Set up a Rust project".to_string(),
            steps: vec![
                PlanStep {
                    description: "cargo init".to_string(),
                    step_type: StepType::Direct,
                    depends_on: vec![],
                    condition: None,
                },
            ],
            verification_criteria: vec!["Cargo.toml exists".to_string()],
            wiki_path: "plans/rust-setup".to_string(),
        };
        let page = plan.to_wiki_page();
        assert_eq!(page.page_type, PageType::Topic);
        assert!(page.body.contains("cargo init"));
        assert!(page.body.contains("Verification Criteria"));
    }

    #[test]
    fn test_parse_plan_with_code_fence() {
        let tool = CreatePlanTool {
            provider: Arc::new(MockProvider), // placeholder — compile-only
            model: "mock".to_string(),
            page_store: Arc::new(MockPageStore), // placeholder
        };
        let raw = r#"```json
{"title":"Test","goal":"G","steps":[],"verification_criteria":[],"wiki_path":""}
```"#;
        let plan = tool.parse_plan(raw, "G").unwrap();
        assert_eq!(plan.title, "Test");
        assert_eq!(plan.wiki_path, "plans/test"); // auto-slugified
    }

    struct MockProvider;
    #[async_trait::async_trait]
    impl LlmProvider for MockProvider {
        async fn chat(&self, _req: ChatRequest) -> anyhow::Result<ChatResponse> {
            unimplemented!()
        }
        fn default_model(&self) -> &str {
            "mock"
        }
    }

    struct MockPageStore;
    #[async_trait::async_trait]
    impl PageStore for MockPageStore {
        async fn write(&self, _page: &WikiPage) -> anyhow::Result<()> {
            Ok(())
        }
    }
}
```

**Note:** The mock tests above are illustrative. If `async_trait` is not available, remove the async trait impls and use `#[tokio::test]` with real dependencies, or skip the mock tests and test `parse_plan` directly (it is synchronous).

---

- [ ] **Step 3: Commit**

```bash
git add gasket/engine/src/tools/create_plan.rs
git commit -m "feat(tools): add create_plan tool for structured task planning

- PlanStep, StepType, Plan structs with JSON serialization
- CreatePlanTool::invoke searches SOPs, calls LLM, persists to wiki
- Plan rendered as PageType::Topic markdown page
- parse_plan handles markdown code fences and auto-generates wiki_path

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

#### Revision from Linus Review: Simplified Step 1 (No JSON AST)

The original Step 1 code below uses `PlanStep`, `StepType`, `Plan` JSON structs and `serde_json` deserialization. **Replace Step 1 entirely** with this simplified version. Also replace Steps 2 and 3.

- [ ] **Step 1 (REVISED): Create simplified `gasket/engine/src/tools/create_plan.rs`**

```rust
//! Tool: create_plan — generate a Markdown execution plan for complex tasks.
//! NO JSON AST — Markdown is the native data structure for LLM-to-LLM communication.

use std::sync::Arc;
use tracing::info;
use crate::wiki::{PageStore, PageType, WikiPage};
use gasket_providers::{ChatMessage, ChatRequest, LlmProvider};

pub struct CreatePlanTool {
    provider: Arc<dyn LlmProvider>,
    model: String,
    page_store: Arc<PageStore>,
}

impl CreatePlanTool {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        model: String,
        page_store: Arc<PageStore>,
    ) -> Self {
        Self { provider, model, page_store }
    }

    pub async fn invoke(
        &self,
        goal: &str,
        context: &[ChatMessage],
    ) -> Result<(String, String), anyhow::Error> {
        let prompt = self.build_plan_prompt(goal, context);

        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage::system(
                    "You are a planning assistant. \
                     Generate a structured execution plan in Markdown format. \
                     Use headers, checklists (- [ ]), and specify dependencies. \
                     Do NOT output JSON."
                ),
                ChatMessage::user(prompt),
            ],
            max_tokens: Some(2048),
            temperature: Some(0.3),
            ..Default::default()
        };

        let response = self.provider.chat(request).await?;
        let plan_markdown = response.content.unwrap_or_default();

        if plan_markdown.is_empty() {
            return Err(anyhow::anyhow!("LLM returned empty plan"));
        }

        // Persist as WikiPage — no JSON AST, just Markdown
        let slug = slugify(goal);
        let path = format!("plans/{}", slug);

        let page = WikiPage::new(
            path.clone(),
            format!("Plan: {}", goal),
            PageType::Topic,
            plan_markdown,
        );

        self.page_store.write(&page).await?;
        info!("create_plan: persisted plan to {}", path);

        let confirmation = format!(
            "Plan created and saved to {}. The agent will now execute each step.",
            path
        );
        Ok((confirmation, path))
    }

    fn build_plan_prompt(&self, goal: &str, context: &[ChatMessage]) -> String {
        let context_text = context
            .iter()
            .map(|m| format!("{:?}: {}", m.role, m.content.as_deref().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "Goal: {}\n\n\
             Recent context:\n{}\n\n\
             Generate a structured execution plan in Markdown. Use:\n\
             - ## headers for phases\n\
             - - [ ] checklists for steps\n\
             - Mark step type inline: [D]irect, [P]arallel/delegated, [?]conditional\n\
             - Include a ## Verification section at the end\n\
             Do NOT output JSON.",
            goal, context_text
        )
    }
}

fn slugify(s: &str) -> String {
    s.to_lowercase()
        .replace(" ", "-")
        .replace("/", "-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect()
}
```

**Replace Step 2 (tests) with:**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Rust Setup"), "rust-setup");
        assert_eq!(slugify("CI/CD Pipeline"), "ci-cd-pipeline");
    }
}
```

**Replace Step 3 (commit) with:**

```bash
git add gasket/engine/src/tools/create_plan.rs
git commit -m "feat(tools): add simplified create_plan tool (Markdown, no JSON AST)

- CreatePlanTool prompts LLM for Markdown plan, persists as WikiPage
- No PlanStep/StepType/Plan JSON structs (KISS)
- Returns confirmation + file path to LLM

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 7: Implement `search_sops` Tool

**Files:**
- Create: `gasket/engine/src/tools/search_sops.rs`
- Modify: `gasket/engine/src/tools/mod.rs`

**Context:** The agent discovers its own SOPs via a tool that queries Tantivy with a `PageType::Sop` filter. This tool is registered in `ToolRegistry` and available to the LLM during any turn.

---

- [ ] **Step 1: Create `gasket/engine/src/tools/search_sops.rs`**

```rust
//! Tool: search_sops — find relevant SOPs by query string via Tantivy.

use crate::wiki::{PageFilter, PageIndex, PageType};

/// Search the wiki for SOP pages relevant to the given query.
///
/// Returns up to `k` hits, ranked by Tantivy BM25.
pub async fn search_sops(
    page_index: &PageIndex,
    query: &str,
    k: usize,
) -> anyhow::Result<Vec<SearchHit>> {
    let mut filter = PageFilter::default();
    filter.page_type = Some(PageType::Sop);
    page_index.search(query, k, Some(filter)).await
}

// Re-export SearchHit if it lives elsewhere; adjust path as needed.
pub use crate::wiki::SearchHit;
```

**Note:** Adjust the `PageIndex::search` signature and `SearchHit` type according to the actual `gasket/engine/src/wiki/` API. The key requirement is passing `PageFilter { page_type: Some(PageType::Sop) }`.

---

- [ ] **Step 2: Register tools in `ToolRegistry`**

In `gasket/engine/src/tools/mod.rs` (or wherever `ToolRegistry` is populated), add:

```rust
pub mod create_plan;
pub mod search_sops;

// During registry initialization:
registry.register("create_plan", Arc::new(CreatePlanTool::new(provider, model, page_store)));
registry.register("search_sops", Arc::new(SearchSopsTool::new(page_index)));
```

**Note:** The exact registration mechanism depends on the existing `ToolRegistry` API. If the registry uses a trait object pattern (e.g. `dyn Tool`), wrap `create_plan::CreatePlanTool` and `search_sops::search_sops` in thin adapter structs that implement the `Tool` trait.

---

- [ ] **Step 3: Commit**

```bash
git add gasket/engine/src/tools/create_plan.rs gasket/engine/src/tools/search_sops.rs gasket/engine/src/tools/mod.rs
git commit -m "feat(tools): register create_plan and search_sops in ToolRegistry

- search_sops queries Tantivy with PageType::Sop filter
- Both tools available to LLM during any turn

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 8: Integrate Checkpoint into MonitoredRunner & End-to-End Test

**Files:**
- Modify: `gasket/engine/src/subagents/monitor.rs`
- Modify: `gasket/engine/src/session/compactor.rs` (ensure `SessionKey: Display`)
- Test: Integration test or manual verification script

**Context:** ~~`ContextCompactor::checkpoint()` returns a summary, but the LLM only benefits if that summary is injected back into the message history. `MonitoredRunner` is the natural place to do this...~~ **REVISED (Linus Review):** Checkpoint is now an optional interceptor on `SteppableExecutor::step()` (added in Task 5 Step 3b), so both `KernelExecutor` and `MonitoredRunner` benefit automatically. This task simplifies to: wire the `ContextCompactor` into `SteppableExecutor` when constructing it in both modes, ensure `SessionKey: Display`, and run end-to-end verification.

---

- [ ] **Step 1 (REVISED): Wire checkpoint callback into both execution modes**

**Since checkpoint is now at SteppableExecutor level (Task 5 Step 3b), this step is simplified.** Instead of adding compactor/event_store fields to `MonitoredRunner`, just pass the checkpoint callback when constructing `SteppableExecutor`:

1. In `KernelExecutor::run_loop()`: pass `steppable.with_checkpoint(callback)` if compactor is configured
2. In `MonitoredSpawner::spawn()`: similarly pass the checkpoint callback

The code blocks below (adding compactor fields to MonitoredRunner, wiring checkpoint in MonitoredRunner::run()) are **superseded by the SteppableExecutor interceptor approach**. Keep as reference only.

~~Original approach (superseded):~~ Modify `MonitoredRunner` in `gasket/engine/src/subagents/monitor.rs`:

1. Add optional compactor field:

```rust
struct MonitoredRunner {
    spec: TaskSpec,
    steppable: SteppableExecutor,
    messages: Vec<ChatMessage>,
    ledger: TokenLedger,
    progress: mpsc::Sender<ProgressUpdate>,
    intervention: mpsc::Receiver<Intervention>,
    max_turns: u32,
    compactor: Option<Arc<ContextCompactor>>,
    event_store: Option<Arc<EventStore>>,
}
```

2. Update `MonitoredRunner::new` to accept an optional compactor:

```rust
    fn new(
        spec: TaskSpec,
        steppable: SteppableExecutor,
        progress: mpsc::Sender<ProgressUpdate>,
        intervention: mpsc::Receiver<Intervention>,
        compactor: Option<Arc<ContextCompactor>>,
        event_store: Option<Arc<EventStore>>,
    ) -> Self {
        // ... existing initialization ...
        Self {
            // ... existing fields ...
            compactor,
            event_store,
        }
    }
```

3. In `MonitoredRunner::run()`, after each `step()` and before the `should_continue` check, add checkpoint logic:

```rust
            // Proactive checkpoint injection
            if let Some(ref compactor) = self.compactor {
                // Build minimal recent events from this turn
                let recent_events = vec![SessionEvent {
                    event_type: "turn".to_string(),
                    content: result.response.content.clone().unwrap_or_default(),
                }];

                // NEVER use turn for checkpoint sequencing. turn is transient and resets on restart.
                // Always derive the sequence anchor from EventStore's actual event increments.
                let session_key = SessionKey::parse(&self.spec.id)
                    .unwrap_or_else(|| SessionKey::new(ChannelType::Cli, &self.spec.id));

                let current_max_sequence = self
                    .event_store
                    .as_ref()
                    .map(|es| async move { es.max_sequence(&session_key).await.unwrap_or(0) })
                    .unwrap_or(async { 0 })
                    .await;

                match compactor.checkpoint(
                    &session_key,
                    current_max_sequence,
                    &recent_events,
                ).await {
                    Ok(Some(summary)) => {
                        self.messages.push(ChatMessage::system(format!(
                            "[Working Memory] {}",
                            summary
                        )));
                        let _ = self
                            .progress
                            .send(ProgressUpdate::ToolStart {
                                name: "checkpoint".to_string(),
                            })
                            .await;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        warn!("Checkpoint failed at sequence {}: {}", current_max_sequence, e);
                    }
                }
            }
```

**Important:** `SessionKey` is a composite key (`struct SessionKey { channel: ChannelType, chat_id: String }`), **NOT** a newtype around `String`. Never implement `From<String>` — it would silently swallow parse failures and pollute the type system with implicit, lossy conversions. Always use `SessionKey::parse()` with an explicit fallback. `SessionKey: Display` (or `ToString`) is required for `save_checkpoint`.

4. Update `MonitoredSpawner::spawn` to pass the compactor through:

```rust
    pub fn spawn(
        provider: Arc<dyn LlmProvider>,
        tools: Arc<ToolRegistry>,
        spec: TaskSpec,
        compactor: Option<Arc<ContextCompactor>>,
        event_store: Option<Arc<EventStore>>,
    ) -> Result<MonitoredHandle, anyhow::Error> {
        // ...
        let handle = tokio::spawn(async move {
            let mut runner = MonitoredRunner::new(spec, steppable, progress_tx, interventor_rx, compactor, event_store);
            // ...
        });
        // ...
    }
```

---

- [ ] **Step 2: Ensure `SessionKey: Display` and reject `From<String>`**

`SessionKey` must implement `Display` (or `ToString`) so that `session_key.to_string()` works for `save_checkpoint`. If missing, add:

```rust
impl std::fmt::Display for SessionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.channel.as_str(), self.chat_id)
    }
}
```

**DO NOT** add `impl From<String> for SessionKey`. `SessionKey` is a composite key; a raw `String` cannot be unambiguously parsed without explicit error handling. In `MonitoredRunner`, use:

```rust
let session_key = SessionKey::parse(&self.spec.id)
    .unwrap_or_else(|| SessionKey::new(ChannelType::Cli, &self.spec.id));
```

---

- [ ] **Step 3: End-to-end verification**

Run a manual or scripted end-to-end test:

```bash
# 1. Build everything
cargo build --workspace

# 2. Run the full test suite
cargo test --workspace

# 3. Run a specific integration scenario (if available)
# Example: start the agent, send a complex multi-step request,
# verify that:
#   - create_plan tool is invoked (check logs)
#   - ProgressUpdate events stream correctly
#   - session_checkpoints table receives rows
#   - sops/ directory gets new pages after task completion
```

**Expected behavior:**
1. User sends a complex task (e.g. "Set up a new Rust project with CI, tests, and docs")
2. Agent calls `create_plan` tool → plan persisted to `plans/{slug}`
3. Agent executes steps turn-by-turn
4. If a subagent is spawned via `spawn_monitored`, progress events stream to the parent
5. Every N sequence increments, `SteppableExecutor` checkpoint interceptor injects `[Working Memory] ...` into messages (both CLI and subagent modes)
6. After task completion, `EvolutionHook` extracts skills and writes a new SOP to `sops/`
7. Next time a similar task arrives, `search_sops` returns the previously learned SOP

---

- [ ] **Step 4: Commit**

```bash
git add gasket/engine/src/subagents/monitor.rs
git commit -m "feat(subagents): integrate proactive checkpointing into MonitoredRunner

- MonitoredRunner accepts optional ContextCompactor
- Calls checkpoint() every N sequence increments and injects summary into messages
- SessionKey requires Display; uses parse() with explicit fallback (no From<String>)

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Summary of Changes

| Task | Module | Key Deliverable |
|------|--------|-----------------|
| 1 | `kernel::executor` | `SteppableExecutor` + `StepResult` extracted; `KernelExecutor` composes internally |
| 2 | `wiki` | `PageType::Sop`, `sops/` directory, Tantivy indexing |
| 3 | `hooks::evolution` | SOP extraction with full template, "No Execution, No Memory" enforcement |
| 4 | `subagents::monitor` | `MonitoredSpawner` + `MonitoredRunner`, channel-based progress/intervention |
| 5 | `session::compactor` + `storage` + `kernel` | `CheckpointConfig`, `session_checkpoints` table, **SteppableExecutor-level** injection (unified) |
| 6 | `tools::create_plan` | Simplified `CreatePlanTool` — **Markdown-based, no JSON AST** |
| 7 | `tools::search_sops` | Tantivy search with `PageType::Sop` filter, registered in `ToolRegistry` |
| 8 | Integration | Wire checkpoint in both `KernelExecutor` and `MonitoredRunner`; end-to-end flow verified |

---

## End of Plan
