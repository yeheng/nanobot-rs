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
    api_key: sk-or-v1-your-key-here
  
  # OpenAI
  openai:
    api_key: sk-your-key
  
  # DeepSeek
  deepseek:
    api_key: sk-your-key
  
  # Zhipu AI
  zhipu:
    api_key: your-key
  
  # Ollama - Local models
  ollama:
    api_base: http://localhost:11434
  
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
    max_iterations: 20
  
  # Multi-model configuration (for dynamic switching)
  models:
    default:
      provider: openrouter
      model: anthropic/claude-4.5-sonnet
    
    fast:
      provider: zhipu
      model: glm-4-flash
    
    coder:
      provider: deepseek
      model: deepseek-chat
    
    reasoning:
      provider: deepseek
      model: deepseek-reasoner
      thinking:
        enabled: true
        budget_tokens: 4000

# ============================================
# 3. Channel Configuration
# ============================================
channels:
  # Telegram
  telegram:
    token: your-bot-token
    allowed_users: []  # Empty allows all users
  
  # Discord
  discord:
    token: your-bot-token
  
  # Slack
  slack:
    app_token: xapp-your-token
    bot_token: xoxb-your-token
  
  # Feishu (Lark)
  feishu:
    app_id: cli-your-id
    app_secret: your-secret
    encrypt_key: your-encrypt-key
  
  # WebSocket
  websocket:
    host: 127.0.0.1
    port: 8080
    auth_token: your-auth-token

# ============================================
# 4. Tools Configuration
# ============================================
tools:
  # Shell execution configuration
  exec:
    # Command policy: allow_all / deny_all / allow_list
    command_policy: allow_list
    
    # Allowed commands (when policy is allow_list)
    allowed_commands:
      - git
      - cargo
      - npm
      - python3
      - curl
      - ls
      - cat
      - grep
    
    # Timeout settings (seconds)
    default_timeout: 30
    max_timeout: 300
  
  # Web tools configuration
  web:
    # Search API
    search:
      provider: brave  # brave, tavily, exa, firecrawl
      api_key: your-key
    
    # Request proxy
    proxy: http://proxy.example.com:8080
  
  # Sandbox configuration (if sandbox feature enabled)
  sandbox:
    enabled: false
    max_memory_mb: 512
    max_cpu_percent: 50

# ============================================
# 5. Storage Configuration
# ============================================
storage:
  # SQLite database path
  database_path: ~/.gasket/gasket.db
  
  # Workspace root directory
  workspace_path: ~/.gasket
  
  # Embedding model configuration (if local-embedding enabled)
  embedding:
    model: BAAI/bge-small-zh-v1.5
    cache_dir: ~/.gasket/.cache/embeddings

# ============================================
# 6. Logging & Monitoring
# ============================================
logging:
  level: info  # debug, info, warn, error
  format: pretty  # pretty, json, compact
  
  # OpenTelemetry configuration (optional)
  otel:
    enabled: false
    endpoint: http://localhost:4317

# ============================================
# 7. Gateway Service Configuration
# ============================================
gateway:
  # Session timeout (seconds)
  session_timeout: 3600
  
  # Maximum concurrent sessions
  max_sessions: 100
  
  # Rate limiting
  rate_limit:
    requests_per_minute: 60
    burst_size: 10

# ============================================
# 8. Heartbeat & Cron
# ============================================
heartbeat:
  # Heartbeat file path
  path: ~/.gasket/HEARTBEAT.md
  
  # Check interval (seconds)
  check_interval: 60
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

| Provider | Type | Description |
|----------|------|-------------|
| openrouter | OpenAI Compatible | Supports Claude, GPT, DeepSeek, etc. |
| openai | OpenAI Compatible | Official API |
| anthropic | OpenAI Compatible | Claude API |
| deepseek | OpenAI Compatible | DeepSeek API |
| zhipu | OpenAI Compatible | Zhipu AI |
| dashscope | OpenAI Compatible | Tongyi Qianwen |
| moonshot | OpenAI Compatible | Moonshot |
| minimax | OpenAI Compatible | MiniMax |
| ollama | OpenAI Compatible | Local models |
| gemini | Gemini | Google API |
| copilot | Copilot | GitHub Copilot |

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
    command_policy: deny_all  # Disable commands in production
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
| `GASKET_VAULT_PASSWORD` | Vault encryption password | - |
| `RUST_LOG` | Log level | Overrides config |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OpenTelemetry endpoint | Overrides config |
| `OPENROUTER_API_KEY` | API key (can be referenced in config) | - |

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
