## Quick Start

```bash
# Build (release mode, all channels)
cargo build --release --workspace

# Run CLI in interactive mode
cargo run --release --package nanobot-cli -- agent

# Single message mode
cargo run --release --package nanobot-cli -- agent -m "your message"

# Start gateway (multi-channel daemon)
cargo run --release --package nanobot-cli -- gateway

# Initialize configuration
cargo run --release --package nanobot-cli -- onboard
```

## Project Structure

```
nanobot-rs/                    # Rust workspace root
├── nanobot-core/              # Core library (all business logic)
│   └── src/
│       ├── agent/             # Agent loop, executor, pipeline
│       ├── bus/               # Actor-based message bus (Router → Session → Outbound)
│       ├── channels/          # Communication channels (Telegram, Discord, Slack, etc.)
│       ├── config/            # Configuration loading
│       ├── mcp/               # MCP protocol client
│       ├── memory/            # SQLite + FTS5 storage
│       ├── providers/         # LLM providers (OpenAI, Anthropic, DeepSeek, etc.)
│       ├── tools/             # Tool system (exec, file, web, spawn_parallel)
│       └── vault/             # Knowledge vault scanner
└── nanobot-cli/               # CLI binary
    └── src/commands/          # Command handlers (agent, gateway, onboard)

web/                           # Vue.js frontend (Vite + Tailwind)
tantivy-mcp/                   # Tantivy search MCP server
docs/                          # Design documentation
```

## Key Files

| File | Purpose |
|------|---------|
| `nanobot-rs/Cargo.toml` | Workspace definition, shared dependencies |
| `~/.nanobot/config.yaml` | Runtime configuration (providers, agents, channels) |
| `config.example.yaml` | Example configuration with model profiles |
| `docs/architecture.md` | Full system architecture |
| `docs/data-flow.md` | Message flow diagrams |

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
- **Agent Loop:** `nanobot-core/src/agent/loop_.rs` is the core execution engine
- **Streaming:** SSE streaming with thinking/reasoning mode support
- **MCP:** JSON-RPC 2.0 over stdio for external tool servers
- **Dynamic Models:** `switch_model` tool allows delegating tasks to specialized models

## Testing

```bash
# Run all tests
cargo test --workspace

# Run with specific feature
cargo test --features "telegram" --package nanobot-core
```

## Gotchas

- Config file is at `~/.nanobot/config.yaml`, not project root
- Use `provider/model` format for model IDs (e.g., `openrouter/anthropic/claude-4.5-sonnet`)
- SQLite is bundled; no separate installation needed
- Feature flags control which channels are compiled (`--features "telegram,discord"`)