# CLAUDE.md
Behavioral guidelines to reduce common LLM coding mistakes. Merge with project-specific instructions as needed.

**Tradeoff:** These guidelines bias toward caution over speed. For trivial tasks, use judgment.

## 1. Think Before Coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:
- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them - don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

## 2. Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

## 3. Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it - don't delete it.

When your changes create orphans:
- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.

## 4. Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:
- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:
```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

---

**These guidelines are working if:** fewer unnecessary changes in diffs, fewer rewrites due to overcomplication, and clarifying questions come before implementation rather than after mistakes.

## 5. Quick Start

```bash
# Build (release mode, all channels)
cargo build --release --workspace

# Run CLI in interactive mode
cargo run --release --package gasket-cli -- agent

# Single message mode
cargo run --release --package gasket-cli -- agent -m "your message"

# Start gateway (multi-channel daemon)
cargo run --release --package gasket-cli -- gateway

# Initialize configuration
cargo run --release --package gasket-cli -- onboard
```

## 6. Workspace Structure

```
gasket/                       # Rust workspace root
├── types/                    # Core types and schemas
├── vault/                    # Knowledge vault scanner
├── storage/                  # SQLite + FTS5 storage (EventStore, SqliteStore)
├── bus/                      # Actor-based message bus (Router → Session → Outbound)
├── engine/                   # Core orchestration (agent loop, tools, hooks)
├── cli/                      # CLI binary and commands
├── providers/                # LLM providers (OpenAI, Anthropic, DeepSeek, etc.)
├── channels/                 # Communication channels (Telegram, Discord, Slack, etc.)
├── sandbox/                  # Code execution sandbox
├── tantivy/                  # Tantivy search MCP server
└── web/                      # Vue.js frontend (Vite + Tailwind)
```

## 7. Key Files

| File | Purpose |
|------|---------|
| `gasket/Cargo.toml` | Workspace definition with 10 member crates |
| `~/.gasket/config.yaml` | Runtime configuration (providers, agents, channels) |
| `config.example.yaml` | Example configuration with model profiles |
| `gasket/engine/src/agent/loop_.rs` | Core agent execution engine |
| `gasket/engine/src/agent/summarization.rs` | Context compression with embeddings |
| `gasket/engine/src/wiki/` | Wiki knowledge system (store, query, ingest, lint) |
| `gasket/engine/src/wiki/query/tantivy_adapter.rs` | Tantivy BM25 full-text search |
| `gasket/engine/src/wiki/lint/` | Wiki health checks (structural + semantic) |
| `gasket/storage/src/wiki/` | SQLite wiki tables (pages, relations, sources, log) |
| `docs/architecture.md` | Full system architecture |
| `docs/data-flow.md` | Message flow diagrams |

## 8. Feature Flags

| Flag | Crate | Purpose |
|------|-------|---------|
| `local-embedding` | storage/engine | ONNX embedding via fastembed |
| `telegram` | channels | Telegram bot support |
| `discord` | channels | Discord bot support |
| `slack` | channels | Slack integration |
| `provider-gemini` | providers | Google Gemini support |
| `provider-copilot` | providers | GitHub Copilot support |

## 9.Code Style

- **Rust Edition:** 2021
- **Max line width:** 100 chars (`rustfmt.toml`)
- **Tab spaces:** 4
- **Async runtime:** tokio
- **Error handling:** thiserror for library, anyhow for CLI

## 10. Environment Variables

| Variable | Purpose |
|----------|---------|
| `RUST_LOG` | Log level (debug, info, warn, error) |
| `OPENAI_API_KEY` | OpenAI provider |
| `ANTHROPIC_API_KEY` | Anthropic provider |
| `ZHIPU_API_KEY` | Zhipu (智谱) provider |
| `DEEPSEEK_API_KEY` | DeepSeek provider |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OpenTelemetry endpoint |
| `OTEL_SDK_DISABLED=true` | Disable OpenTelemetry |

## 11. Architecture Notes

- **Actor Model:** Three-actor pipeline: Router → Session → Outbound (zero-lock)
- **Agent Loop:** `engine/src/agent/loop_.rs` is the core execution engine
- **Streaming:** SSE streaming with thinking/reasoning mode support
- **MCP:** JSON-RPC 2.0 over stdio for external tool servers
- **Dynamic Models:** `switch_model` tool allows delegating tasks to specialized models
- **Engine facade:** `engine` crate re-exports bus, channels, providers, storage

## 12. Wiki Knowledge System

The wiki-first knowledge system replaces the old `memory/` module with a three-layer architecture:

| Layer | Storage | Purpose |
|-------|---------|---------|
| Raw Sources | `~/.gasket/sources/` | Original documents |
| Compiled Wiki | `~/.gasket/wiki/` (SQLite + optional .md cache) | Structured knowledge pages |
| Search Index | `~/.gasket/wiki/.tantivy/` | Tantivy BM25 full-text search |

**Three operations:** Ingest (add knowledge), Query (retrieve knowledge), Lint (health check)

**Key structs:** `PageStore` (CRUD), `PageIndex` (Tantivy search), `WikiLinter` (lint checks), `WikiQueryEngine` (three-phase retrieval)

**Tool backward compatibility:** `memorize` → PageStore.write(), `memory_search` → PageIndex.search(), `memory_refresh` → PageIndex.rebuild()

**Wiki CLI commands:** `gasket wiki init`, `gasket wiki ingest <path>`, `gasket wiki search <query>`, `gasket wiki list`, `gasket wiki lint`, `gasket wiki stats`, `gasket wiki migrate`

## 13. Testing

```bash
# Run all tests
cargo test --workspace

# Run with specific feature
cargo test --features "telegram" --package gasket-channels
```

## 14. Gotchas

- Config file is at `~/.gasket/config.yaml`, not project root
- Use `provider/model` format for model IDs (e.g., `openrouter/anthropic/claude-4.5-sonnet`)
- SQLite is bundled; no separate installation needed
- Feature flags control which channels are compiled (`--features "telegram,discord"`)