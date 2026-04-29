# 配置指南

> Gasket-RS 完整配置说明

---

## 配置文件位置

```
~/.gasket/config.yaml    # 主配置文件
```

首次使用请运行：

```bash
gasket onboard    # 自动创建配置和工作空间
```

---

## 最小可用配置

```yaml
providers:
  openrouter:
    api_key: sk-or-v1-your-key-here

agents:
  defaults:
    model: openrouter/anthropic/claude-4.5-sonnet
```

---

## 完整配置示例

```yaml
# ============================================
# 1. LLM 提供商配置
# ============================================
providers:
  # OpenRouter - 推荐，支持多模型
  openrouter:
    api_base: "https://openrouter.ai/api/v1"
    api_key: "${OPENROUTER_API_KEY}"

  # OpenAI
  openai:
    api_base: "https://api.openai.com/v1"
    api_key: "${OPENAI_API_KEY}"

  # Anthropic - Claude 原生 API
  anthropic:
    api_base: "https://api.anthropic.com/v1"
    api_key: "${ANTHROPIC_API_KEY}"

  # DeepSeek
  deepseek:
    api_base: "https://api.deepseek.com/v1"
    api_key: "${DEEPSEEK_API_KEY}"

  # 智谱 AI
  zhipu:
    api_base: "https://open.bigmodel.cn/api/paas/v4"
    api_key: "${ZHIPU_API_KEY}"

  # Ollama - 本地模型
  ollama:
    api_base: "http://localhost:11434/v1"

  # Google Gemini - 原生 API
  gemini:
    api_base: "https://generativelanguage.googleapis.com/v1beta"
    api_key: "${GOOGLE_API_KEY}"

  # GitHub Copilot
  copilot:
    # 使用 OAuth 或 PAT，见 copilot-setup.md

# ============================================
# 2. Agent 配置
# ============================================
agents:
  # 默认配置
  defaults:
    model: openrouter/anthropic/claude-4.5-sonnet
    temperature: 0.7
    max_tokens: 4096
    max_iterations: 100
    memory_window: 10
    thinking_enabled: false
    historyRecallK: 5
    streaming: true
    # WebSocket 子代理摘要长度限制（0 = 不限，默认）
    ws_summary_limit: 0

    # 可选：覆盖内部 AI 行为提示词模板
    # prompts:
    #   identity_prefix: "You are a cat. Meow.\nYour working directory: {workspace}."
    #   summarization: "Summarize the following conversation in bullet points."
    #   checkpoint: "Summarize current task state for working memory."
    #   evolution: "Extract memories from this conversation.\n\n{{conversation}}"
    #   planning: "Create a plan for: {{goal}}\n\nContext:\n{{context}}"

    # 可选：三阶段记忆 Token 预算（默认值如下）
    # memory_budget:
    #   bootstrap: 1500    # 阶段 1：Profile + Active Hot/Warm
    #   scenario: 1500     # 阶段 2：场景特定 Hot + 标签匹配 Warm
    #   on_demand: 1000    # 阶段 3：语义搜索填充
    #   total_cap: 4000    # 全阶段硬上限

  # 多模型配置（用于动态切换）
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
# 3. 渠道配置
# ============================================
channels:
  # Telegram
  telegram:
    enabled: false
    token: ""
    allow_from: []  # 空数组允许所有用户

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

  # 飞书
  wechat:
    enabled: false
    allowFrom: []

# ============================================
# 4. 工具配置
# ============================================
tools:
  # 限制文件操作仅在工作空间内
  restrict_to_workspace: false

  # Web 工具配置
  web:
    search_provider: brave        # brave, tavily, exa, firecrawl
    brave_api_key: "your-key"
    # tavily_api_key: "your-key"
    # exa_api_key: "your-key"
    # firecrawl_api_key: "your-key"
    # http_proxy: "http://127.0.0.1:7890"
    # https_proxy: "http://127.0.0.1:7890"
    # socks5_proxy: "socks5://127.0.0.1:1080"
    use_env_proxy: true           # 自动读取 HTTP_PROXY 等环境变量

  # Shell 执行配置
  exec:
    timeout: 120                  # 默认超时（秒）
    workspace: "."                # 命令执行工作目录

    # 沙箱配置（可选，当前仅支持 bwrap）
    sandbox:
      enabled: false
      backend: bwrap
      tmp_size_mb: 64

    # 命令策略
    policy:
      allowlist: []               # 允许的命令，空数组 = 允许所有
      denylist: ["rm -rf /", "mkfs"]  # 拒绝的命令模式

    # 资源限制
    limits:
      max_memory_mb: 512
      max_cpu_secs: 60
      max_output_bytes: 1048576   # 1 MB

# ============================================
# 5. 嵌入与语义召回
# ============================================
# 需要: cargo build --features embedding
embedding:
  provider:
    type: Api
    endpoint: "https://api.openai.com/v1/embeddings"
    model: "text-embedding-3-small"
    api_key: "${OPENAI_API_KEY}"
    dim: 1536

  # 替代方案: 本地 ONNX（无需 API 密钥，完全离线运行）
  # 需要: cargo build --features "embedding local-onnx"
  # provider:
  #   type: LocalOnnx
  #   model: "BGESmallENV15"
  #   dim: 384
  #   cache_dir: "/path/to/embedding_cache"

  recall:
    top_k: 5
    token_budget: 500
    min_score: 0.3

  # 内存热索引上限（0 = 纯 SQLite，不使用内存索引）
  hot_limit: 1000

# ============================================
# 6. 其他配置
# ============================================
# 可选：外部停用词文件路径（用于基于关键词的历史召回）
# stop_words_path: "~/.gasket/stop_words.txt"
```

---

## 配置项详解

### 模型格式

```yaml
model: provider/model

# 示例
model: openrouter/anthropic/claude-4.5-sonnet
model: deepseek/deepseek-chat
model: zhipu/glm-5
```

### Provider 类型

Provider 名称会自动决定原生实现，通常**不需要**手动设置 `provider_type`：

| Provider 名称 | 原生协议 | 说明 |
|-------------|---------|------|
| `openai` | OpenAI Compatible | 官方 API |
| `anthropic` / `claude` | Anthropic Messages API | Claude 原生 API |
| `deepseek` | OpenAI Compatible | DeepSeek API |
| `zhipu` | OpenAI Compatible | 智谱 AI |
| `openrouter` | OpenAI Compatible | 多模型网关 |
| `ollama` | OpenAI Compatible | 本地模型 |
| `litellm` | OpenAI Compatible | 本地代理 |
| `gemini` | Gemini | Google API |
| `copilot` | Copilot | GitHub Copilot |
| `moonshot` / `kimi` | Moonshot API | 月之暗面 |
| `minimax` / `minimaxi` | MiniMax API | MiniMax |

仅当使用自定义名称（如 `my-proxy`）且系统无法识别时，才需要显式设置 `provider_type`。

### Agent 配置项

| 配置项 | 类型 | 默认值 | 说明 |
|--------|------|--------|------|
| `model` | string | - | 默认模型，格式 `provider/model` |
| `temperature` | float | 0.7 | 采样温度，越低越确定 |
| `max_tokens` | int | 4096 | 单次回复最大 Token 数 |
| `max_iterations` | int | 100 | 单轮对话最大工具调用次数 |
| `memory_window` | int | 10 | 加载到上下文的最近消息数 |
| `thinking_enabled` | bool | false | 启用深度思考模式（仅支持 reasoning 模型） |
| `streaming` | bool | true | 启用流式输出 |
| `historyRecallK` | int | 5 | 语义历史召回条数 |
| `ws_summary_limit` | int | 0 | WebSocket 子代理摘要长度限制（字符数），0 = 不限 |
| `memory_budget` | object | - | 三阶段记忆 Token 预算 |
| `prompts` | object | - | 覆盖内部 AI 行为提示词模板 |

### Model Profile 配置项

| 配置项 | 类型 | 说明 |
|--------|------|------|
| `provider` | string | 提供商名称 |
| `model` | string | 模型名称 |
| `description` | string | 模型描述（用于 AI 自主切换时理解） |
| `capabilities` | string[] | 能力标签（如 `["code", "reasoning"]`） |
| `temperature` | float | 该模型的温度覆盖 |
| `max_tokens` | int | 该模型的最大 Token 覆盖 |
| `thinking_enabled` | bool | 该模型是否启用思考模式 |

### Embedding 提供商类型

| 类型 | 说明 | 所需特性 |
|------|------|----------|
| Api | OpenAI 兼容 HTTP API（OpenAI、Ollama 等） | `embedding` |
| LocalOnnx | 本地 ONNX 模型（通过 fastembed） | `embedding local-onnx` |

### 温度参数 (temperature)

| 值 | 用途 |
|----|------|
| 0.0 - 0.3 | 代码生成、数学计算（确定性强） |
| 0.4 - 0.7 | 通用对话、问答（平衡） |
| 0.8 - 1.2 | 创意写作、头脑风暴（多样性高） |

---

## 多环境配置

### 开发环境

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

### 生产环境

```yaml
# ~/.gasket/config.prod.yaml
providers:
  openrouter:
    api_key: ${OPENROUTER_API_KEY}  # 从环境变量读取

agents:
  defaults:
    model: openrouter/anthropic/claude-4.5-sonnet
    temperature: 0.7

tools:
  exec:
    policy:
      allowlist: []  # 生产环境禁用命令执行
```

使用：

```bash
GASKET_CONFIG=~/.gasket/config.prod.yaml gasket gateway
```

---

## 环境变量

| 变量 | 说明 | 优先级 |
|------|------|--------|
| `GASKET_CONFIG` | 配置文件路径 | 最高 |
| `GASKET_MASTER_PASSWORD` | Vault 加密密码 | - |
| `RUST_LOG` | 日志级别（如 `info`、`debug`） | 覆盖配置 |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OpenTelemetry 端点 | 覆盖配置 |
| `OPENROUTER_API_KEY` | API Key（可被 config 引用） | - |
| `ANTHROPIC_API_KEY` | Anthropic API Key | - |
| `DEEPSEEK_API_KEY` | DeepSeek API Key | - |
| `ZHIPU_API_KEY` | 智谱 API Key | - |
| `OPENAI_API_KEY` | OpenAI API Key | - |

---

## 配置验证

```bash
# 验证配置格式
gasket config validate

# 查看当前配置
gasket config show

# 测试 LLM 连接
gasket config test-connection
```
