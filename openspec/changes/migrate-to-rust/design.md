## Context

nanobot 是一个用 Python 编写的轻量级 AI 助手框架，核心代码约 7,618 行。项目采用异步架构，使用 LiteLLM 作为 LLM 网关，支持多种聊天渠道（Telegram、Discord、Slack 等）。

### 当前架构
```
nanobot/
├── agent/          # 核心代理逻辑 (loop, context, memory, tools)
├── channels/       # 聊天渠道集成 (11 个渠道)
├── providers/      # LLM 提供商 (通过 LiteLLM)
├── bus/            # 消息总线 (inbound/outbound)
├── config/         # 配置管理
├── cli/            # 命令行接口
├── cron/           # 定时任务
├── session/        # 会话管理
└── utils/          # 工具函数
```

### 迁移约束
- 配置文件格式 (`~/.nanobot/config.yaml`) 保持兼容
- CLI 命令接口必须保持兼容
- 工作区目录结构保持不变

## Goals / Non-Goals

### Goals
- 完全用 Rust 重写核心功能
- 保持与现有配置和 CLI 的向后兼容
- 提升性能（启动时间、内存占用）
- 保持代码简洁（目标 < 10,000 行 Rust 代码）

### Non-Goals
- 不重写 `bridge/` 目录（TypeScript WhatsApp 桥接）
- 不改变用户工作区结构

## Decisions

### 1. 项目结构

**决定**: 采用 Cargo workspace 结构

```
nanobot-rs/
├── Cargo.toml              # Workspace 定义
├── nanobot-core/           # 核心库
│   ├── Cargo.toml
│   └── src/
│       ├── agent/
│       ├── tools/
│       ├── providers/
│       ├── channels/
│       ├── bus/
│       ├── config/
│       ├── session/
│       └── lib.rs
├── nanobot-cli/            # CLI 应用
│   ├── Cargo.toml
│   └── src/
│       └── main.rs
└── nanobot-macros/         # 过程宏 (可选)
    └── ...
```

**理由**: 
- 分离核心库和 CLI，便于测试和复用
- 遵循 Rust 社区最佳实践

**替代方案**:
- 单 crate：结构简单但不利于模块化
- 多 repo：增加维护成本

### 2. 异步运行时

**决定**: 使用 `tokio`

**理由**:
- Rust 异步生态的事实标准
- 丰富的生态系统 (tokio-util, tokio-stream 等)
- 所有渠道库都支持 tokio

### 3. LLM API 调用

**决定**: 自实现 OpenAI 兼容 API 客户端

**理由**:
- LiteLLM 是 Python 特有的
- OpenAI API 已成为行业标准
- 实现简单，只需 `reqwest` + `serde`

```rust
// 示例结构
pub struct LlmClient {
    http: reqwest::Client,
    api_base: String,
    api_key: String,
}

pub async fn chat(&self, request: ChatRequest) -> Result<ChatResponse> {
    // 实现 OpenAI 兼容 API 调用
}
```

**替代方案**:
- `async-openai` crate：功能完善但可能有额外依赖

### 4. 工具系统

**决定**: 使用 trait 定义工具接口

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;
    async fn execute(&self, args: serde_json::Value) -> Result<String>;
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}
```

**理由**:
- 类型安全
- 易于扩展
- 与 Python 版本接口类似

### 5. 配置管理

**决定**: 使用 `serde` + `serde_json`

```rust
#[derive(Debug, Deserialize)]
pub struct Config {
    pub providers: HashMap<String, ProviderConfig>,
    pub agents: AgentsConfig,
    pub channels: ChannelsConfig,
    #[serde(default)]
    pub tools: ToolsConfig,
}
```

**理由**:
- 与现有 JSON 配置完全兼容
- 编译时类型检查

### 6. 渠道集成策略

**决定**: 优先使用成熟的 Rust crate

| 渠道 | Rust Crate | 成熟度 |
|------|-----------|--------|
| Telegram | `teloxide` | 高 |
| Discord | `serenity` / `twilight` | 高 |
| Slack | `slack-morphism` | 中 |
| Email | `lettre` + `imap` | 高 |
| WhatsApp | 保留 bridge (TypeScript) | - |
| Feishu | 自实现 HTTP API | - |
| DingTalk | 自实现 HTTP API | - |

**理由**:
- 利用现有生态减少开发量
- 成熟 crate 更稳定

### 7. MCP 支持

**决定**: Phase 4 实现，使用 `rmcp` crate 或自实现

**理由**:
- MCP 是较新的协议
- 可以后期添加
- 核心功能优先

## Risks / Trade-offs

### 风险

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|----------|
| 渠道库不完整 | 中 | 高 | 先实现核心渠道，其他后续添加 |
| 开发时间超预期 | 中 | 中 | 分阶段交付，每阶段可独立使用 |
| 性能不达预期 | 低 | 中 | 性能测试在 Phase 1 后进行 |
| 社区接受度低 | 中 | 高 | 保持简单设计，详细文档 |

### Trade-offs

| 选择 | 优点 | 缺点 |
|------|------|------|
| 自实现 LLM 客户端 | 轻量、可控 | 需要维护 |
| 分阶段迁移 | 风险可控、可验证 | 需要维护两套代码一段时间 |
| 保留 WhatsApp bridge | 快速可用 | 多语言依赖 |

## Migration Plan

### Phase 1: 核心框架 (Week 1-2)

**目标**: 可用的 agent loop + tools

```bash
# 验证命令
nanobot agent -m "What is 2+2?"
```

**交付物**:
- [ ] `nanobot-core/src/agent/loop.rs`
- [ ] `nanobot-core/src/agent/context.rs`
- [ ] `nanobot-core/src/agent/memory.rs`
- [ ] `nanobot-core/src/tools/` (read, write, edit, list, shell)
- [ ] `nanobot-core/src/providers/base.rs`
- [ ] `nanobot-core/src/providers/openai.rs`
- [ ] `nanobot-core/src/config/`

### Phase 2: CLI & Config (Week 3)

**目标**: 完整 CLI 体验

```bash
# 验证命令
nanobot onboard
nanobot status
nanobot agent
```

**交付物**:
- [ ] `nanobot-cli/src/main.rs`
- [ ] 配置加载和验证
- [ ] 交互式 CLI 模式

### Phase 3: Channels (Week 4-5)

**目标**: 支持主流渠道

```bash
# 验证命令
nanobot gateway  # Telegram + Discord
```

**交付物**:
- [ ] `nanobot-core/src/channels/telegram.rs`
- [ ] `nanobot-core/src/channels/discord.rs`
- [ ] `nanobot-core/src/channels/base.rs`
- [ ] `nanobot-core/src/bus/`

### Phase 4: 高级功能 (Week 6)

**目标**: 完整功能集

**交付物**:
- [ ] MCP 支持
- [ ] Cron 定时任务
- [ ] Heartbeat 心跳
- [ ] Web 搜索工具

### Phase 5: 发布 (Week 7)

**目标**: 生产就绪

**交付物**:
- [ ] 单元测试和集成测试
- [ ] 文档更新
- [ ] GitHub Actions CI/CD
- [ ] Docker 镜像
- [ ] crates.io 发布

## Rollback Plan

如果迁移失败，可以：

1. **回退到 Python 版本**: Git 保留完整历史
2. **混合模式**: Python 版本处理不支持的渠道
3. **逐步迁移**: 只在 Rust 版本稳定后才弃用 Python

## Open Questions

1. **Python 扩展支持**: 是否需要通过 `pyo3` 提供 Python 绑定？
   - 建议：Phase 5 再决定
   
2. **版本策略**: Rust 版本如何与 Python 版本共存？
   - 建议：使用 `v2.0.0` 标识 Rust 版本

3. **用户迁移指南**: 需要多少文档支持？
   - 建议：保持配置兼容，用户无需修改配置文件
