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
    api_key: sk-or-v1-your-key-here
  
  # OpenAI
  openai:
    api_key: sk-your-key
  
  # DeepSeek
  deepseek:
    api_key: sk-your-key
  
  # 智谱 AI
  zhipu:
    api_key: your-key
  
  # Ollama - 本地模型
  ollama:
    api_base: http://localhost:11434
  
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
    max_iterations: 20
  
  # 多模型配置（用于动态切换）
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
# 3. 渠道配置
# ============================================
channels:
  # Telegram
  telegram:
    token: your-bot-token
    allowed_users: []  # 空数组允许所有用户
  
  # Discord
  discord:
    token: your-bot-token
  
  # Slack
  slack:
    app_token: xapp-your-token
    bot_token: xoxb-your-token
  
  # 飞书
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
# 4. 工具配置
# ============================================
tools:
  # Shell 执行配置
  exec:
    # 命令策略：allow_all / deny_all / allow_list
    command_policy: allow_list
    
    # 允许的命令列表（当 policy 为 allow_list）
    allowed_commands:
      - git
      - cargo
      - npm
      - python3
      - curl
      - ls
      - cat
      - grep
    
    # 超时设置（秒）
    default_timeout: 30
    max_timeout: 300
  
  # Web 工具配置
  web:
    # 搜索 API
    search:
      provider: brave  # brave, tavily, exa, firecrawl
      api_key: your-key
    
    # 请求代理
    proxy: http://proxy.example.com:8080
  
  # 沙箱配置（如启用 sandbox 功能）
  sandbox:
    enabled: false
    max_memory_mb: 512
    max_cpu_percent: 50

# ============================================
# 5. 存储配置
# ============================================
storage:
  # SQLite 数据库路径
  database_path: ~/.gasket/gasket.db
  
  # 工作空间根目录
  workspace_path: ~/.gasket
  
  # 嵌入模型配置（如启用 local-embedding 功能）
  embedding:
    model: BAAI/bge-small-zh-v1.5
    cache_dir: ~/.gasket/.cache/embeddings

# ============================================
# 6. 日志与监控
# ============================================
logging:
  level: info  # debug, info, warn, error
  format: pretty  # pretty, json, compact
  
  # OpenTelemetry 配置（可选）
  otel:
    enabled: false
    endpoint: http://localhost:4317

# ============================================
# 7. Gateway 服务配置
# ============================================
gateway:
  # 会话超时（秒）
  session_timeout: 3600
  
  # 最大并发会话数
  max_sessions: 100
  
  # 速率限制
  rate_limit:
    requests_per_minute: 60
    burst_size: 10

# ============================================
# 8. 心跳与定时任务
# ============================================
heartbeat:
  # 心跳文件路径
  path: ~/.gasket/HEARTBEAT.md
  
  # 检查间隔（秒）
  check_interval: 60
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

| Provider | 类型 | 说明 |
|----------|------|------|
| openrouter | OpenAI Compatible | 支持 Claude, GPT, DeepSeek 等 |
| openai | OpenAI Compatible | 官方 API |
| anthropic | OpenAI Compatible | Claude API |
| deepseek | OpenAI Compatible | DeepSeek API |
| zhipu | OpenAI Compatible | 智谱 AI |
| dashscope | OpenAI Compatible | 通义千问 |
| moonshot | OpenAI Compatible | Moonshot |
| minimax | OpenAI Compatible | MiniMax |
| ollama | OpenAI Compatible | 本地模型 |
| gemini | Gemini | Google API |
| copilot | Copilot | GitHub Copilot |

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
    command_policy: deny_all  # 生产环境禁用命令执行
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
| `GASKET_VAULT_PASSWORD` | Vault 加密密码 | - |
| `RUST_LOG` | 日志级别 | 覆盖配置 |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OpenTelemetry 端点 | 覆盖配置 |
| `OPENROUTER_API_KEY` | API Key（可被 config 引用） | - |

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
