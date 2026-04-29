# Configuration Guide

> Complete Gasket-RS Configuration Reference

---

## Configuration File Location

```
~/.gasket/config.yaml    # Main configuration file
```

First time setup:

```bash
gasket onboard    # Automatically creates config and workspace
```

---

## Minimal Working Config

```yaml
providers:
  openrouter:
    api_key: sk-or-v1-your-key-here

agents:
  defaults:
    model: openrouter/anthropic/claude-4.5-sonnet
```

---

## Full Configuration Example

```yaml
# ============================================
# 1. LLM Provider Configuration
# ============================================
providers:
  # OpenRouter - Recommended, supports multiple models
  openrouter:
    api_base: "https://openrouter.ai/api/v1"
    api_key: "${OPENROUTER_API_KEY}"

  # OpenAI
  openai:
    api_base: "https://api.openai.com/v1"
    api_key: "${OPENAI_API_KEY}"

  # Anthropic - Claude native API
  anthropic:
    api_base: "https://api.anthropic.com/v1"
    api_key: "${ANTHROPIC_API_KEY}"

  # DeepSeek
  deepseek:
    api_base: "https://api.deepseek.com/v1"
    api_key: "${DEEPSEEK_API_KEY}"

  # Zhipu AI
  zhipu:
    api_base: "https://open.bigmodel.cn/api/paas/v4"
    api_key: "${ZHIPU_API_KEY}"

  # Ollama - Local models
  ollama:
    api_base: "http://localhost:11434/v1"

  # Google Gemini - native API
  gemini:
    api_base: "https://generativelanguage.googleapis.com/v1beta"
    api_key: "${GOOGLE_API_KEY}"

  # GitHub Copilot
  copilot:
    # Use OAuth or PAT, see copilot-setup.md

# ============================================
# 2. Agent Configuration
# ============================================
agents:
  # Default configuration
  defaults:
    model: openrouter/anthropic/claude-4.5-sonnet
    temperature: 0.7
    max_tokens: 4096
    max_iterations: 100
    memory_window: 10
    thinking_enabled: false
    historyRecallK: 5
    streaming: true
    # WebSocket subagent summary length limit (0 = unlimited, default)
    ws_summary_limit: 0

    # Optional: override internal AI behavior prompt templates
    # prompts:
    #   identity_prefix: "You are a cat. Meow.\nYour working directory: {workspace}."
    #   summarization: "Summarize the following conversation in bullet points."
    #   checkpoint: "Summarize current task state for working memory."
    #   evolution: "Extract memories from this conversation.\n\n{{conversation}}"
    #   planning: "Create a plan for: {{goal}}\n\nContext:\n{{context}}"

    # Optional: three-phase memory token budgets (defaults shown)
    # memory_budget:
    #   bootstrap: 1500    # Phase 1: profile + active hot/warm
    #   scenario: 1500     # Phase 2: scenario-specific hot + tag-matched warm
    #   on_demand: 1000    # Phase 3: semantic search fill
    #   total_cap: 4000    # Hard upper limit across all phases

  # Multi-model configuration (for dynamic switching)
  models:
    default:
      provider: openrouter
      model: anthropic/claude-4.5-sonnet
      description: "General-purpose model for everyday tasks."
      capabilities: ["general", "chat"]
      temperature: 0.7

    fast:
      provider: zhipu
      model: glm-4-flash
      description: "Fast responses for simple queries"
      capabilities: ["fast", "chat"]
      max_tokens: 2048

    coder:
      provider: deepseek
      model: deepseek-coder
      description: "Specialized for code generation and debugging"
      capabilities: ["code", "reasoning"]
      temperature: 0.3
      thinking_enabled: true

    reasoning:
      provider: deepseek
      model: deepseek-reasoner
      description: "Advanced reasoning for complex problems"
      capabilities: ["reasoning", "math", "analysis"]
      thinking_enabled: true
      max_tokens: 8192

# ============================================
# 3. Channel Configuration
# ============================================
channels:
  # Telegram
  telegram:
    enabled: false
    token: ""
    allow_from: []  # Empty allows all users

  # Discord
  discord:
    enabled: false
    token: ""
    allow_from: []

  # Slack
  slack:
    enabled: false
    bot_token: ""
    app_token: ""
    allow_from: []

  # WebSocket
  websocket:
    enabled: true

  # WeChat
  wechat:
    enabled: false
    allowFrom: []

# ============================================
# 4. Tools Configuration
# ============================================
tools:
  # Restrict file operations to workspace
  restrict_to_workspace: false

  # Web tools configuration
  web:
    search_provider: brave        # brave, tavily, exa, firecrawl
    brave_api_key: "your-key"
    # tavily_api_key: "your-key"
    # exa_api_key: "your-key"
    # firecrawl_api_key: "your-key"
    # http_proxy: "http://127.0.0.1:7890"
    # https_proxy: "http://127.0.0.1:7890"
    # socks5_proxy: "socks5://127.0.0.1:1080"
    use_env_proxy: true           # Auto-read HTTP_PROXY etc.

  # Shell execution configuration
  exec:
    timeout: 120                  # Default timeout (seconds)
    workspace: "."                # Command execution working directory

    # Sandbox configuration (optional, currently only bwrap)
    sandbox:
      enabled: false
      backend: bwrap
      tmp_size_mb: 64

    # Command policy
    policy:
      allowlist: []               # Allowed commands, empty = allow all
      denylist: ["rm -rf /", "mkfs"]  # Denied command patterns

    # Resource limits
    limits:
      max_memory_mb: 512
      max_cpu_secs: 60
      max_output_bytes: 1048576   # 1 MB

# ============================================
# 5. Embedding & Semantic Recall
# ============================================
# Requires: cargo build --features embedding
embedding:
  provider:
    type: Api
    endpoint: "https://api.openai.com/v1/embeddings"
    model: "text-embedding-3-small"
    api_key: "${OPENAI_API_KEY}"
    dim: 1536

  # Alternative: Local ONNX (no API key, runs offline)
  # Requires: cargo build --features "embedding local-onnx"
  # provider:
  #   type: LocalOnnx
  #   model: "BGESmallENV15"
  #   dim: 384
  #   cache_dir: "/path/to/embedding_cache"

  recall:
    top_k: 5
    token_budget: 500
    min_score: 0.3

  # In-memory hot index limit (0 = pure SQLite, no memory index)
  hot_limit: 1000

# ============================================
# 6. Other Configuration
# ============================================
# Optional: external stop-words file for keyword-based history recall
# stop_words_path: "~/.gasket/stop_words.txt"
```

---

## Configuration Options Explained

### Model Format

```yaml
model: provider/model

# Examples
model: openrouter/anthropic/claude-4.5-sonnet
model: deepseek/deepseek-chat
model: zhipu/glm-5
```

### Provider Types

Provider names automatically determine the native implementation. You usually **do not** need to set `provider_type`:

| Provider Name | Native Protocol | Description |
|-------------|---------|------|
| `openai` | OpenAI Compatible | Official API |
| `anthropic` / `claude` | Anthropic Messages API | Claude native API |
| `deepseek` | OpenAI Compatible | DeepSeek API |
| `zhipu` | OpenAI Compatible | Zhipu AI |
| `openrouter` | OpenAI Compatible | Multi-model gateway |
| `ollama` | OpenAI Compatible | Local models |
| `litellm` | OpenAI Compatible | Local proxy |
| `gemini` | Gemini | Google API |
| `copilot` | Copilot | GitHub Copilot |
| `moonshot` / `kimi` | Moonshot API | Moonshot |
| `minimax` / `minimaxi` | MiniMax API | MiniMax |

Only set `provider_type` if you use a custom name (e.g., `my-proxy`) that the system cannot recognize.

### Agent Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `model` | string | - | Default model, format `provider/model` |
| `temperature` | float | 0.7 | Sampling temperature, lower = more deterministic |
| `max_tokens` | int | 4096 | Max tokens per response |
| `max_iterations` | int | 100 | Max tool calls per turn |
| `memory_window` | int | 10 | Recent messages loaded into context |
| `thinking_enabled` | bool | false | Enable deep thinking mode (reasoning models only) |
| `streaming` | bool | true | Enable streaming output |
| `historyRecallK` | int | 5 | Semantic history recall count |
| `ws_summary_limit` | int | 0 | WebSocket subagent summary length limit (chars), 0 = unlimited |
| `memory_budget` | object | - | Three-phase memory token budget |
| `prompts` | object | - | Override internal AI behavior prompt templates |

### Model Profile Options

| Option | Type | Description |
|--------|------|-------------|
| `provider` | string | Provider name |
| `model` | string | Model name |
| `description` | string | Model description (for AI auto-switching) |
| `capabilities` | string[] | Capability tags (e.g., `["code", "reasoning"]`) |
| `temperature` | float | Temperature override for this model |
| `max_tokens` | int | Max tokens override for this model |
| `thinking_enabled` | bool | Enable thinking mode for this model |

### Embedding Provider Types

| Type | Description | Requires |
|------|-------------|----------|
| Api | OpenAI-compatible HTTP API (OpenAI, Ollama, etc.) | `embedding` feature |
| LocalOnnx | Local ONNX model via fastembed | `embedding local-onnx` features |

### Temperature Parameter

| Value | Use Case |
|-------|----------|
| 0.0 - 0.3 | Code generation, math (highly deterministic) |
| 0.4 - 0.7 | General chat, Q&A (balanced) |
| 0.8 - 1.2 | Creative writing, brainstorming (high diversity) |

---

## Multi-Environment Configuration

### Development Environment

```yaml
# ~/.gasket/config.dev.yaml
providers:
  ollama:
    api_base: http://localhost:11434

agents:
  defaults:
    model: ollama/llama3.2
    temperature: 0.8
```

### Production Environment

```yaml
# ~/.gasket/config.prod.yaml
providers:
  openrouter:
    api_key: ${OPENROUTER_API_KEY}  # Read from environment

agents:
  defaults:
    model: openrouter/anthropic/claude-4.5-sonnet
    temperature: 0.7

tools:
  exec:
    policy:
      allowlist: []  # Disable commands in production
```

Usage:

```bash
GASKET_CONFIG=~/.gasket/config.prod.yaml gasket gateway
```

---

## Environment Variables

| Variable | Description | Priority |
|----------|-------------|----------|
| `GASKET_CONFIG` | Configuration file path | Highest |
| `GASKET_MASTER_PASSWORD` | Vault encryption password | - |
| `RUST_LOG` | Log level (e.g., `info`, `debug`) | Overrides config |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OpenTelemetry endpoint | Overrides config |
| `OPENROUTER_API_KEY` | API Key (can be referenced in config) | - |
| `ANTHROPIC_API_KEY` | Anthropic API Key | - |
| `DEEPSEEK_API_KEY` | DeepSeek API Key | - |
| `ZHIPU_API_KEY` | Zhipu API Key | - |
| `OPENAI_API_KEY` | OpenAI API Key | - |

---

## Configuration Validation

```bash
# Validate configuration format
gasket config validate

# Display current configuration
gasket config show

# Test LLM connection
gasket config test-connection
```
