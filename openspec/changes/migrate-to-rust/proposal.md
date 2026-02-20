# Change: Migrate nanobot from Python to Rust

## Why

nanobot 是一个超轻量级 AI 助手框架，当前核心代码约 7,618 行 Python。迁移到 Rust 可以带来以下好处：

1. **性能提升**：Rust 的零成本抽象和无 GC 特性可显著降低内存占用和启动时间
2. **类型安全**：Rust 的强类型系统可在编译期捕获更多错误
3. **并发安全**：Rust 的所有权模型保证线程安全，无需担心数据竞争
4. **单二进制部署**：无需 Python 运行时，分发更简单
5. **长期维护**：Rust 生态稳定，适合构建长期项目

## What Changes

### **BREAKING** 核心重写
- 将 `nanobot/` 目录下的 Python 代码重写为 Rust
- 使用 `tokio` 作为异步运行时
- 使用 `serde` 进行序列化/反序列化
- 使用 `reqwest` 处理 HTTP 请求
- 使用 `pyo3` 保留 Python 扩展能力（可选）

### 模块映射

| Python 模块 | Rust Crate/模块 | 说明 |
|------------|-----------------|------|
| `agent/loop.py` | `nanobot-core/src/agent/loop.rs` | 核心代理循环 |
| `agent/context.py` | `nanobot-core/src/agent/context.rs` | 上下文构建 |
| `agent/memory.py` | `nanobot-core/src/agent/memory.rs` | 内存存储 |
| `agent/tools/` | `nanobot-core/src/tools/` | 工具实现 |
| `bus/` | `nanobot-core/src/bus/` | 消息总线 |
| `channels/` | `nanobot-core/src/channels/` | 渠道集成 |
| `providers/` | `nanobot-core/src/providers/` | LLM 提供商 |
| `config/` | `nanobot-core/src/config/` | 配置管理 |
| `cli/` | `nanobot-cli/` | CLI 入口 |
| `cron/` | `nanobot-core/src/cron/` | 定时任务 |
| `session/` | `nanobot-core/src/session/` | 会话管理 |

### 依赖迁移

| Python 依赖 | Rust 替代 | 说明 |
|------------|-----------|------|
| `litellm` | 自实现 + `reqwest` | LiteLLM 风格的 LLM API 调用 |
| `pydantic` | `serde` | 数据验证和序列化 |
| `typer` | `clap` | CLI 框架 |
| `websockets` | `tokio-tungstenite` | WebSocket 客户端 |
| `httpx` | `reqwest` | HTTP 客户端 |
| `loguru` | `tracing` | 日志框架 |
| `croniter` | `cron` | Cron 解析 |
| `python-telegram-bot` | `teloxide` | Telegram Bot |
| `slack-sdk` | `slack-morphism` | Slack API |

### 保留兼容性
- 配置文件格式 `~/.nanobot/config.yaml` (从 JSON 迁移到 YAML)
- 工作区目录结构保持不变
- CLI 命令接口保持兼容

## Impact

### 受影响的规范
- `nanobot-core` (新建)

### 受影响的代码
- **全部重写**: `nanobot/` 目录下所有 Python 代码
- **保留**: `workspace/` 用户工作区（无需修改）
- **更新**: `pyproject.toml` → `Cargo.toml`
- **更新**: `Dockerfile`（基础镜像变更）

### 风险与缓解

| 风险 | 缓解措施 |
|------|----------|
| 渠道集成复杂度高 | 使用现成 Rust crate (teloxide, slack-morphism 等) |
| MCP 协议支持 | 使用 `rmcp` crate 或实现 MCP 客户端 |
| 开发周期长 | 分阶段迁移，每个阶段都可独立运行 |
| 社区熟悉度低 | 保持代码简洁，提供详细文档 |

### 迁移阶段

1. **Phase 1: 核心框架** - agent loop, tools, providers
2. **Phase 2: CLI & Config** - 命令行接口，配置管理
3. **Phase 3: Channels** - Telegram, Discord, Slack 等渠道
4. **Phase 4: 高级功能** - MCP, cron, heartbeat
5. **Phase 5: 发布** - 文档、测试、发布

## Timeline

- **Week 1-2**: Phase 1 核心框架
- **Week 3**: Phase 2 CLI & Config
- **Week 4-5**: Phase 3 Channels
- **Week 6**: Phase 4 高级功能
- **Week 7**: Phase 5 发布准备

## Success Criteria

- [ ] 所有现有 CLI 命令可用
- [ ] 至少支持 Telegram 和 Discord 渠道
- [ ] 支持主流 LLM 提供商 (OpenAI, Anthropic, OpenRouter)
- [ ] 内存占用降低 50%+
- [ ] 启动时间 < 100ms
