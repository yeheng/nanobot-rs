## Quick Start

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

## Workspace Structure

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

## Key Files

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

## Feature Flags

| Flag | Crate | Purpose |
|------|-------|---------|
| `local-embedding` | storage/engine | ONNX embedding via fastembed |
| `telegram` | channels | Telegram bot support |
| `discord` | channels | Discord bot support |
| `slack` | channels | Slack integration |
| `provider-gemini` | providers | Google Gemini support |
| `provider-copilot` | providers | GitHub Copilot support |

## Code Style

- **Rust Edition:** 2021
- **Max line width:** 100 chars (`rustfmt.toml`)
- **Tab spaces:** 4
- **Async runtime:** tokio
- **Error handling:** thiserror for library, anyhow for CLI

## Environment Variables

| Variable | Purpose |
|----------|---------|
| `RUST_LOG` | Log level (debug, info, warn, error) |
| `OPENAI_API_KEY` | OpenAI provider |
| `ANTHROPIC_API_KEY` | Anthropic provider |
| `ZHIPU_API_KEY` | Zhipu (智谱) provider |
| `DEEPSEEK_API_KEY` | DeepSeek provider |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OpenTelemetry endpoint |
| `OTEL_SDK_DISABLED=true` | Disable OpenTelemetry |

## Architecture Notes

- **Actor Model:** Three-actor pipeline: Router → Session → Outbound (zero-lock)
- **Agent Loop:** `engine/src/agent/loop_.rs` is the core execution engine
- **Streaming:** SSE streaming with thinking/reasoning mode support
- **MCP:** JSON-RPC 2.0 over stdio for external tool servers
- **Dynamic Models:** `switch_model` tool allows delegating tasks to specialized models
- **Engine facade:** `engine` crate re-exports bus, channels, providers, storage

## Wiki Knowledge System

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

## Testing

```bash
# Run all tests
cargo test --workspace

# Run with specific feature
cargo test --features "telegram" --package gasket-channels
```

## Gotchas

- Config file is at `~/.gasket/config.yaml`, not project root
- Use `provider/model` format for model IDs (e.g., `openrouter/anthropic/claude-4.5-sonnet`)
- SQLite is bundled; no separate installation needed
- Feature flags control which channels are compiled (`--features "telegram,discord"`)