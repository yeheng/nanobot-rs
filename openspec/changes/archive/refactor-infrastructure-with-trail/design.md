# Design: Trail System and Infrastructure Refactoring

## Context

nanobot 是一个 Rust 实现的 AI 助手框架，当前包含以下核心组件：
- **Providers**: LLM API 调用抽象（OpenAI, Gemini, etc.）
- **Channels**: 消息渠道集成（Telegram, Discord, Slack, etc.）
- **Tools**: Agent 可用工具（shell, filesystem, web, etc.）
- **Memory**: 长期记忆存储
- **MessageBus**: 组件间消息传递

当前架构的问题：
1. **缺乏统一的可观测性**：日志分散，难以调试复杂交互
2. **扩展性不足**：添加功能需要修改核心代码
3. **接口不统一**：每个组件有不同的生命周期管理方式
4. **测试困难**：组件耦合导致单元测试需要大量 mock

## Goals / Non-Goals

### Goals
- ✅ 引入 Trail 系统提供端到端的执行追踪
- ✅ 通过中间件模式实现无侵入式扩展
- ✅ 统一所有组件的接口设计模式
- ✅ 解耦组件依赖，提高可测试性
- ✅ 支持 OpenTelemetry 集成（可选）

### Non-Goals
- ❌ 完全重写所有现有代码（渐进式重构）
- ❌ 支持分布式追踪（暂限于单进程）
- ❌ 替换所有第三方库（如 teloxide, reqwest）
- ❌ 提供 UI 界面（仅提供数据接口）

## Decisions

### Decision 1: Trail 系统设计

**选择**: 基于 span 和 event 的层级追踪模型

**理由**:
- 与 OpenTelemetry 模型一致，便于未来集成
- Rust 生态中 `tracing` crate 提供良好支持
- 支持结构化日志和异步上下文传播

**核心类型**:
```rust
// Trail 系统
pub trait Trail: Send + Sync {
    fn start_span(&self, name: &str, attrs: Vec<(String, Value)>) -> SpanId;
    fn end_span(&self, span_id: SpanId);
    fn record_event(&self, name: &str, attrs: Vec<(String, Value)>);
    fn current_context(&self) -> TrailContext;
}

// Span 标识符
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpanId(u64);

// 异步上下文传播
#[derive(Debug, Clone)]
pub struct TrailContext {
    pub trace_id: TraceId,
    pub span_id: SpanId,
    pub baggage: HashMap<String, String>,
}
```

**替代方案考虑**:
1. ❌ 简单日志：无法表达层级关系和上下文传播
2. ❌ 事件溯源：过于复杂，不符合当前需求
3. ✅ Span/Event 模型：平衡了表达能力和复杂度

### Decision 2: 中间件模式

**选择**: 洋葱模型（Onion Model）+ async trait

**理由**:
- 允许在操作前后执行逻辑
- 支持提前返回（短路）
- 与 Tower Service 模式兼容

**接口设计**:
```rust
// Provider 中间件
#[async_trait]
pub trait ProviderMiddleware: Send + Sync {
    async fn handle(
        &self,
        request: ChatRequest,
        next: Next<'_>,
    ) -> anyhow::Result<ChatResponse>;
}

// Tool 中间件
#[async_trait]
pub trait ToolMiddleware: Send + Sync {
    async fn handle(
        &self,
        ctx: ExecutionContext,
        args: Value,
        next: Next<'_>,
    ) -> ToolResult;
}
```

**替代方案考虑**:
1. ❌ 观察者模式：无法修改请求/响应
2. ❌ 责任链模式：难以表达"中间"逻辑
3. ✅ 洋葱模型：灵活且符合直觉

### Decision 3: 统一的 Builder 模式

**选择**: 所有组件使用 Builder 配置

**理由**:
- 支持复杂配置而不污染构造函数
- 类型安全的配置验证
- 便于添加新配置项

**示例**:
```rust
let provider = OpenAIProvider::builder()
    .api_key(env::var("OPENAI_API_KEY")?)
    .model("gpt-4")
    .middleware(LoggingMiddleware::new())
    .middleware(RetryMiddleware::new(3))
    .trail(trail.clone())
    .build()?;
```

### Decision 4: Memory 存储抽象

**选择**: Trait-based 抽象 + 多种实现

**理由**:
- 支持不同的存储后端需求
- 便于测试（可用内存存储）
- 允许用户自定义存储

**接口**:
```rust
#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn read(&self, key: &str) -> Result<Option<String>>;
    async fn write(&self, key: &str, value: &str) -> Result<()>;
    async fn delete(&self, key: &str) -> Result<()>;
    async fn query(&self, query: MemoryQuery) -> Result<Vec<MemoryEntry>>;
}

// 实现
pub struct FileStorage { /* ... */ }
pub struct MemoryStorage { /* ... */ }
#[cfg(feature = "redis")]
pub struct RedisStorage { /* ... */ }
```

## Architecture

### 组件层次关系

```
┌─────────────────────────────────────────────────┐
│                   Agent Loop                     │
│  ┌──────────────┐         ┌──────────────────┐  │
│  │   Provider   │────────▶│  Trail System    │  │
│  │ + Middleware │         │  - Span tracking │  │
│  └──────────────┘         │  - Context prop  │  │
│                           │  - Event logging │  │
│  ┌──────────────┐         └──────────────────┘  │
│  │   Channel    │───────────────┐               │
│  │ + Middleware │               │               │
│  └──────────────┘               ▼               │
│                           ┌──────────────┐      │
│  ┌──────────────┐         │   MessageBus │      │
│  │    Tool      │────────▶│ + Trail ctx  │      │
│  │ + Middleware │         └──────────────┘      │
│  └──────────────┘               │               │
│                                 ▼               │
│  ┌──────────────┐         ┌──────────────┐      │
│  │    Memory    │────────▶│MemoryStore   │      │
│  │ + Middleware │         │ impl         │      │
│  └──────────────┘         └──────────────┘      │
└─────────────────────────────────────────────────┘
```

### Trail 上下文传播流程

```
1. 用户消息到达 Channel
   └─> Channel.start_span("message_received")
       └─> TrailContext::current()

2. Agent Loop 处理消息
   └─> Agent.start_span("agent_process", parent_ctx)
       └─> Tool.start_span("tool_execute", parent_ctx)
           └─> Provider.start_span("llm_call", parent_ctx)
               └─> Trail 记录完整的调用链

3. 响应返回
   └─> 所有 span 依次结束
       └─> Trail 输出完整的执行树
```

### 中间件执行顺序

```rust
// 请求流向（从外到内）
LoggingMiddleware::handle
└─> MetricsMiddleware::handle
    └─> RetryMiddleware::handle
        └─> 实际 Provider::chat

// 响应流向（从内到外）
实际 Provider::chat 返回
└─> RetryMiddleware 处理（如需重试）
    └─> MetricsMiddleware 记录指标
        └─> LoggingMiddleware 记录响应
```

## Trade-offs

### Trail 系统开销

**优点**: 提供完整的可观测性和调试能力  
**缺点**: 增加内存和 CPU 开销  
**缓解**:
- 提供采样配置（如仅追踪 10% 请求）
- 支持动态开关（运行时启用/禁用）
- 使用异步写入避免阻塞主流程

### 中间件复杂度

**优点**: 灵活扩展，符合开闭原则  
**缺点**: 增加代码复杂度，调试困难  
**缓解**:
- 提供清晰的中间件文档和示例
- Trail 系统帮助可视化中间件执行链
- 限制最大中间件数量（编译时检查）

### API 破坏性变更

**优点**: 获得更清晰、更统一的接口  
**缺点**: 用户需要迁移代码  
**缓解**:
- 提供兼容层（deprecated 标记旧接口）
- 详细的迁移文档和示例
- 分阶段迁移（每个 Phase 可独立完成）

## Open Questions

1. **Trail 数据存储**: Trail 数据应该保存在哪里？内存/文件/外部系统？
   - 建议默认内存，提供插件接口支持外部存储

2. **采样策略**: 如何配置采样率？全局还是按组件？
   - 建议全局配置，但允许 span 级别覆盖

3. **中间件顺序**: 用户如何控制中间件执行顺序？
   - Builder 模式中按添加顺序执行，支持 `insert_before/after`

4. **性能监控**: 是否需要内置性能基准测试？
   - 建议在 CI 中添加性能回归测试

## Migration Plan

### 阶段划分

| Phase | 内容 | 依赖 | 持续时间 |
|-------|------|------|----------|
| 1 | Trail 系统核心 | 无 | 3 天 |
| 2 | Provider 重构 | Phase 1 | 5 天 |
| 3 | Channel 重构 | Phase 1 | 5 天 |
| 4 | Tool 重构 | Phase 1 | 4 天 |
| 5 | Memory 重构 | Phase 1 | 3 天 |
| 6 | 集成测试 | Phase 2-5 | 3 天 |

### 兼容性保证

- **Phase 1-5**: 保留旧接口，标记 deprecated
- **Phase 6**: 移除旧接口，发布 major version
- 提供自动化迁移脚本（如 `sed` 替换）

### Rollback 策略

- 每个 Phase 完成后创建 git tag
- 发现问题可快速回滚到上一个 tag
- 保留旧接口直到 Phase 6 完成

## References

- [OpenTelemetry Specification](https://opentelemetry.io/docs/reference/specification/)
- [Tower Service Pattern](https://docs.rs/tower/latest/tower/)
- [Rust tracing crate](https://docs.rs/tracing/latest/tracing/)
- [Middleware Pattern in Rust](https://blog.yoshuawuyts.com/middleware-patterns-in-rust/)
