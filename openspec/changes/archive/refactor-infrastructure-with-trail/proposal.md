# Change: Refactor Infrastructure with Trail System

## Why

当前 nanobot 的基础设施（provider/channel/tool/memory）存在以下问题：

1. **缺乏统一的可观测性**：各个组件的日志、监控分散，难以追踪完整的执行链路
2. **扩展性不足**：添加新功能需要修改核心代码，无法通过插件化方式扩展
3. **组件耦合度高**：各模块之间存在直接依赖，难以独立测试和替换
4. **接口不统一**：不同组件的生命周期管理、错误处理模式不一致

引入 **Trail 系统**（执行轨迹追踪）并结合中间件模式，可以：
- 提供端到端的可观测性（类似 OpenTelemetry）
- 通过中间件机制实现无侵入式扩展
- 统一组件接口设计，降低学习成本
- 支持异步上下文传播，便于调试和性能分析

## What Changes

### **BREAKING** 核心架构变更

#### 1. Trail 系统核心
- 引入 `Trail` trait：记录执行轨迹、spans、events
- 引入 `TrailContext`：异步上下文传播机制
- 引入 `TrailMiddleware` trait：中间件接口
- 实现内置中间件：Logging、Metrics、ErrorTracking

#### 2. Provider 系统重构
- **MODIFIED** `LlmProvider` trait：增加 `TrailContext` 参数
- **ADDED** `ProviderMiddleware` trait：支持请求/响应拦截
- **ADDED** `ProviderBuilder`：支持中间件链式配置
- **REFACTORED** `ProviderRegistry`：支持动态注册和发现

#### 3. Channel 系统重构
- **MODIFIED** `Channel` trait：统一生命周期管理（init/start/stop/graceful_shutdown）
- **ADDED** `ChannelMiddleware` trait：消息处理拦截器
- **ADDED** `MessageContext`：包含 Trail 信息和元数据
- **REFACTORED** `MessageBus`：集成 Trail 系统

#### 4. Tool 系统重构
- **MODIFIED** `Tool` trait：增加 `ExecutionContext`（包含 TrailContext）
- **ADDED** `ToolMiddleware` trait：执行前/后钩子
- **ADDED** `ToolMetadata`：描述工具能力、权限要求等
- **REFACTORED** `ToolRegistry`：支持分类、标签、权限检查

#### 5. Memory 系统重构
- **REFACTORED** `MemoryStore` trait：抽象存储接口
- **ADDED** 多种存储后端：FileStorage、MemoryStorage、RedisStorage（可选）
- **ADDED** `MemoryMiddleware` trait：读写拦截器
- **ADDED** `MemoryQuery`：结构化查询接口

### 架构模式统一
所有组件遵循统一模式：
1. **Core Trait**：定义核心能力
2. **Middleware Trait**：定义拦截器接口
3. **Builder Pattern**：配置和构建实例
4. **Trail Integration**：集成执行轨迹追踪

## Impact

### 受影响的规范
- **NEW**: `trail-system` - Trail 系统核心
- **MODIFIED**: `provider-registry` - Provider 注册和管理
- **MODIFIED**: `channel-system` - Channel 消息处理
- **MODIFIED**: `tool-registry` - Tool 注册和执行
- **MODIFIED**: `memory-store` - Memory 存储抽象

### 受影响的代码
- `nanobot-core/src/trail/` (新建)
- `nanobot-core/src/providers/` (重构)
- `nanobot-core/src/channels/` (重构)
- `nanobot-core/src/tools/` (重构)
- `nanobot-core/src/agent/memory.rs` → `nanobot-core/src/memory/` (重构)
- `nanobot-core/src/bus/` (集成 Trail)
- `nanobot-core/src/agent/loop.rs` (使用新接口)

### 迁移路径

#### Phase 1: Trail 系统基础
- 实现 `Trail` trait 和基础类型
- 实现 `TrailContext` 和异步传播
- 实现内置中间件

#### Phase 2: Provider 重构
- 重构 `LlmProvider` trait
- 实现 `ProviderMiddleware` 和 Builder
- 更新现有 providers (OpenAI, Gemini, etc.)
- 迁移 `ProviderRegistry`

#### Phase 3: Channel 重构
- 重构 `Channel` trait
- 实现 `ChannelMiddleware`
- 更新现有 channels (Telegram, Discord, etc.)
- 集成 Trail 到 `MessageBus`

#### Phase 4: Tool 重构
- 重构 `Tool` trait
- 实现 `ToolMiddleware` 和 Metadata
- 更新现有 tools (shell, filesystem, etc.)
- 迁移 `ToolRegistry`

#### Phase 5: Memory 重构
- 抽象 `MemoryStore` trait
- 实现多种存储后端
- 更新 agent loop 使用新接口

#### Phase 6: 集成和测试
- 更新所有 agent 逻辑
- 编写迁移文档
- 性能基准测试

### 风险与缓解

| 风险 | 缓解措施 |
|------|----------|
| 大规模重构影响稳定性 | 分 6 个 Phase，每个 Phase 独立测试 |
| Trail 性能开销 | 提供采样配置，默认仅在 debug 模式启用详细追踪 |
| 迁移成本高 | 提供兼容层，渐进式迁移 |
| API 复杂度增加 | 提供默认配置和 Builder 模式简化使用 |

## Success Criteria

- [ ] Trail 系统可追踪完整的 agent 执行链路（从消息接收到响应发送）
- [ ] 所有组件支持中间件扩展，无需修改核心代码
- [ ] 添加新 provider/channel/tool 的代码量减少 50%+
- [ ] 提供统一的监控面板（基于 Trail 数据）
- [ ] 性能开销 < 5%（在采样模式下）
- [ ] 完整的迁移文档和示例代码
