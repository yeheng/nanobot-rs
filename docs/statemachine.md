# 多 Agent 协作状态机 (三省六部)

> Nanobot 多 Agent 分层协作框架使用指南

---

## 概述

State Machine 子系统为 nanobot 引入了**数据驱动的多 Agent 协作机制**，灵感来自中国古代的「三省六部」治理体系。它提供：

- **状态机引擎** — 事件驱动的状态转换，严格的生命周期管理
- **配置驱动** — 通过 YAML/JSON 定义任意状态机拓扑，不再是硬编码
- **权限控制** — 基于角色的状态访问控制，Agent 只能处理授权状态
- **停滞检测** — 自动发现超时任务并触发恢复策略
- **完全 opt-in** — `state_machine.enabled: false`（默认）时零开销

---

## 架构

```
用户请求
    ↓
[StateMachineEngine] ← StateMachineEvent (mpsc channel)
    ↓ dispatch
[Triage] → [Planning] ⇄ [Review] → [Dispatch] → [Ministry₁..₆]
   太子       中书省      门下省       尚书省       六部执行层
    ↓                                                  ↓
  分析分类                                         执行 + 进度上报
```

### 三层架构

| 层次 | 角色 | 职责 | 调度模式 |
|------|------|------|----------|
| **分诊层** | 太子 (taizi) | 分析请求、分类优先级 | 同步 |
| **治理层** | 中书省 (zhongshu) | 战略规划、任务分解 | 同步 |
| | 门下省 (menxia) | 审核质量门控，可拒绝打回 | 同步 |
| | 尚书省 (shangshu) | 任务调度分发 | 同步 |
| **执行层** | 六部 (ministry) | 执行具体任务 | 异步 |

### 调度模式

- **治理层 Agent**（taizi、zhongshu、menxia、shangshu）使用 `submit_and_wait()` — 同步等待决策结果
- **执行层 Agent**（ministry）使用 `submit()` — 异步 fire-and-forget，通过 `report_progress` 工具上报进度

---

## 快速开始

### 1. 启用 State Machine

在 `~/.nanobot/config.yaml` 中添加：

```yaml
state_machine:
  enabled: true
```

这会使用默认的三省六部配置启动状态机。

### 2. 验证启动

启动 nanobot 后，日志中应出现：

```
State machine subsystem initialized (initial_state="triage", terminal_states={"done"})
State machine engine started
Stall detector started (timeout=60s, check_interval=20s)
```

Agent 工具列表中会新增两个工具：
- `state_machine_task` — 任务看板
- `report_progress` — 进度上报

---

## 配置参考

### 最小配置

```yaml
state_machine:
  enabled: true
```

### 完整配置

```yaml
state_machine:
  # 主开关 — false 时整个子系统完全休眠
  enabled: true

  # 配置文件路径 (YAML 或 JSON)
  config_path: "~/.nanobot/state_machine.yaml"

  # SOUL 模板目录路径
  soul_templates_path: "~/.nanobot/souls"

  # 是否使用内置三省六部预设 (默认 true)
  use_default_template: true
```

### 状态机配置文件格式

#### 方式 1：在主配置中直接定义

```yaml
state_machine:
  enabled: true
  config_path: "~/.nanobot/state_machine.yaml"
```

#### 方式 2：使用独立配置文件

创建 `~/.nanobot/state_machine.yaml`：

```yaml
# ~/.nanobot/state_machine.yaml

# 初始状态 - 任务创建后进入的第一个状态
initial_state: triage

# 终端状态 - 到达这些状态的任务视为完成
terminal_states: [done]

# 活跃状态 - 需要停滞检测的状态
active_states: [triage, planning, executing, review]

# 同步角色 - 使用同步调度模式的角色
sync_roles: [taizi, zhongshu, menxia, shangshu]

# 状态转换定义
transitions:
  - from: pending
    to: triage
  - from: triage
    to: planning
  - from: planning
    to: reviewing
  - from: reviewing
    to: assigned
  - from: reviewing
    to: planning  # 拒绝打回
  - from: assigned
    to: executing
  - from: executing
    to: review
  - from: executing
    to: blocked
  - from: review
    to: done
  - from: review
    to: blocked
  - from: blocked
    to: executing
  - from: blocked
    to: planning

# 状态 → 角色映射
state_roles:
  pending: system
  triage: taizi
  planning: zhongshu
  reviewing: menxia
  assigned: shangshu
  executing: ministry
  review: menxia
  done: system
  blocked: shangshu

# 审核门控配置
gates:
  reviewing:
    reject_to: blocked  # 超过 max_reviews 后转移到的状态

# 最大审核轮次
max_reviews: 3

# 停滞超时 (秒)
stall_timeout_secs: 60
```

### 配置字段说明

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `initial_state` | string | - | 任务创建后进入的初始状态 |
| `terminal_states` | string[] | `[]` | 终端状态列表 |
| `active_states` | string[] | `[]` | 需要停滞检测的活跃状态 |
| `sync_roles` | string[] | `[]` | 使用同步调度的角色 |
| `transitions` | Transition[] | `[]` | 状态转换列表 |
| `state_roles` | map | `{}` | 状态到角色的映射 |
| `gates` | map | `{}` | 审核门控配置 |
| `max_reviews` | u32 | `3` | 最大审核轮次 |
| `stall_timeout_secs` | u64 | `60` | 停滞超时阈值 |

---

## 任务状态机

### 状态流转图

```
Pending → Triage → Planning → Reviewing ──→ Assigned → Executing → Review → Done
                      ↑          │                         │          │
                      └──────────┘                         │          │
                      (拒绝打回)                            ↓          │
                                                       Blocked ← ← ← ←┘
                                                         │
                                    ┌────────────────────┤
                                    ↓                    ↓
                                 Executing            Planning
                                 (恢复执行)           (恢复规划)
```

### 状态说明

| 状态 | 负责角色 | 说明 |
|------|---------|------|
| `pending` | system | 新创建，等待进入状态机 |
| `triage` | taizi | 太子正在分析分类 |
| `planning` | zhongshu | 中书省正在制定计划 |
| `reviewing` | menxia | 门下省正在审核 |
| `assigned` | shangshu | 尚书省正在分派 |
| `executing` | ministry | 六部正在执行 |
| `review` | menxia | 执行后审核 |
| `done` | system | 已完成 |
| `blocked` | shangshu | 被阻塞，等待恢复 |

### 合法转换表

| 从 | 到 | 触发条件 |
|----|-----|---------|
| pending | triage | 任务创建后自动 |
| triage | planning | 太子完成分析 |
| planning | reviewing | 中书省提交计划 |
| reviewing | assigned | 门下省批准 |
| reviewing | planning | 门下省拒绝打回 |
| assigned | executing | 尚书省分派完成 |
| executing | review | 执行完成，提交审核 |
| executing | blocked | 执行遇阻 |
| review | done | 审核通过 |
| review | blocked | 审核发现问题 |
| blocked | executing | 恢复执行 |
| blocked | planning | 恢复到规划阶段 |

---

## 工具使用指南

### state_machine_task — 任务看板

任务看板是 Agent 与共享任务板交互的主要接口。

#### 创建任务

```json
{
  "action": "create",
  "title": "实现用户认证模块",
  "description": "需要添加 JWT 认证支持",
  "priority": "high",
  "origin_channel": "telegram",
  "origin_chat_id": "12345"
}
```

返回示例：
```json
{
  "status": "created",
  "task_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

#### 查询任务

```json
{
  "action": "get",
  "task_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

#### 列出任务

按状态筛选：
```json
{
  "action": "list",
  "state": "executing"
}
```

按角色筛选：
```json
{
  "action": "list",
  "role": "ministry"
}
```

列出所有：
```json
{
  "action": "list"
}
```

#### 状态转换

```json
{
  "action": "transition",
  "task_id": "550e8400-e29b-41d4-a716-446655440000",
  "to_state": "planning",
  "agent_role": "taizi",
  "reason": "已完成分诊，任务为开发类，优先级 high"
}
```

返回示例：
```json
{
  "status": "transitioned",
  "task_id": "550e8400-e29b-41d4-a716-446655440000",
  "from": "triage",
  "to": "planning"
}
```

#### 查询流转日志

```json
{
  "action": "flow_log",
  "task_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

返回示例：
```json
[
  {
    "id": 1,
    "task_id": "550e8400-e29b-41d4-a716-446655440000",
    "from_state": "pending",
    "to_state": "triage",
    "agent_role": "system",
    "reason": "auto dispatch on creation",
    "timestamp": "2026-03-09T10:00:00Z"
  },
  {
    "id": 2,
    "task_id": "550e8400-e29b-41d4-a716-446655440000",
    "from_state": "triage",
    "to_state": "planning",
    "agent_role": "taizi",
    "reason": "已完成分诊",
    "timestamp": "2026-03-09T10:01:00Z"
  }
]
```

### report_progress — 进度上报

执行层 Agent 定期调用，同时更新心跳时间戳。

```json
{
  "task_id": "550e8400-e29b-41d4-a716-446655440000",
  "agent_role": "ministry",
  "content": "已完成数据库 schema 设计，正在实现 API 层",
  "percentage": 60.0
}
```

返回示例：
```json
{
  "status": "reported",
  "task_id": "550e8400-e29b-41d4-a716-446655440000",
  "content": "已完成数据库 schema 设计，正在实现 API 层",
  "percentage": 60.0
}
```

**重要**：执行层 Agent 必须每 30 秒内至少上报一次进度，否则会被停滞检测标记。

---

## 停滞检测与恢复

### 检测机制

`StallDetector` 每 20-30 秒扫描一次活跃任务（状态在 `active_states` 中定义），检查 `last_heartbeat` 是否超过 `stall_timeout_secs`。

### 恢复策略

| 级别 | 条件 | 动作 |
|------|------|------|
| L1 重试 | 首次停滞 (retry_count=0) | 更新心跳，重新调度同一角色 Agent |
| L2 阻塞 | 重试后仍停滞 (retry_count≥1) | 转为 Blocked 状态，等待手动干预 |

### 手动恢复

Blocked 状态的任务可通过 `state_machine_task` 工具恢复：

```json
{
  "action": "transition",
  "task_id": "550e8400-e29b-41d4-a716-446655440000",
  "to_state": "executing",
  "agent_role": "shangshu",
  "reason": "管理员手动恢复执行"
}
```

---

## SQLite 表结构

State Machine 使用三张独立表，与现有 session/kv 表共享同一数据库：

### state_machine_tasks

```sql
CREATE TABLE state_machine_tasks (
    id              TEXT PRIMARY KEY,       -- UUID v4
    title           TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    state           TEXT NOT NULL DEFAULT 'pending',
    priority        TEXT NOT NULL DEFAULT 'normal',
    assigned_role   TEXT,
    review_count    INTEGER NOT NULL DEFAULT 0,
    retry_count     INTEGER NOT NULL DEFAULT 0,
    last_heartbeat  TEXT NOT NULL,          -- RFC 3339
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    result          TEXT,                   -- 完成时的结果
    origin_channel  TEXT,                   -- 来源渠道
    origin_chat_id  TEXT,                   -- 来源会话
    session_id      TEXT                    -- 会话 ID
);

CREATE INDEX idx_state_machine_tasks_state ON state_machine_tasks(state);
CREATE INDEX idx_state_machine_tasks_role ON state_machine_tasks(assigned_role);
```

### state_machine_flow_log

```sql
CREATE TABLE state_machine_flow_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id     TEXT NOT NULL,
    from_state  TEXT NOT NULL,
    to_state    TEXT NOT NULL,
    agent_role  TEXT NOT NULL,
    reason      TEXT,
    timestamp   TEXT NOT NULL,
    FOREIGN KEY (task_id) REFERENCES state_machine_tasks(id) ON DELETE CASCADE
);

CREATE INDEX idx_state_machine_flow_log_task ON state_machine_flow_log(task_id);
```

### state_machine_progress_log

```sql
CREATE TABLE state_machine_progress_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id     TEXT NOT NULL,
    agent_role  TEXT NOT NULL,
    content     TEXT NOT NULL,
    percentage  REAL,
    timestamp   TEXT NOT NULL,
    FOREIGN KEY (task_id) REFERENCES state_machine_tasks(id) ON DELETE CASCADE
);

CREATE INDEX idx_state_machine_progress_task ON state_machine_progress_log(task_id);
```

**乐观锁**：`update_task_state()` 使用 `WHERE state = :expected` 防止并发冲突。

---

## SOUL 模板

每个角色可以有一个 SOUL.md 文件定义其身份、工具权限、工作流和约束。

### 加载 SOUL 模板

在配置中指定模板目录：

```yaml
state_machine:
  enabled: true
  soul_templates_path: "~/.nanobot/souls"
```

目录中的 `*.md` 文件会被自动加载，文件名（不含扩展名）作为角色名。

### 自定义 SOUL 模板

创建自定义角色模板，例如 `~/.nanobot/souls/developer.md`：

```markdown
# 角色：开发工程师

## 职责
- 执行具体的代码开发任务
- 编写单元测试
- 提交代码审查

## 可用工具
- state_machine_task: 查看和转换任务状态
- report_progress: 上报工作进度

## 工作流
1. 从 assigned 状态获取任务
2. 执行开发工作
3. 定期上报进度
4. 完成后转换到 review 状态
```

---

## 模块结构

```
nanobot-rs/nanobot-statemachine/
├── Cargo.toml
└── src/
    ├── lib.rs              # 模块入口 + bootstrap 函数
    ├── types.rs            # State, Transition, StateMachineConfig
    ├── models.rs           # StateMachineTask, FlowLogEntry, ProgressEntry
    ├── store.rs            # SQLite 持久化层 (StateMachineStore)
    ├── engine.rs           # StateMachineEngine 事件循环
    ├── events.rs           # StateMachineEvent 事件类型
    ├── config_loader.rs    # YAML/JSON 配置加载 + Builder
    ├── stall_detector.rs   # StallDetector 停滞检测
    └── tools/
        ├── mod.rs
        ├── state_machine_task.rs  # 任务看板工具
        └── report_progress.rs     # 进度上报工具
```

---

## 配置示例合集

更多示例配置文件请参考 [examples/](examples/) 目录：

- [state_machine_sansheng.yaml](examples/state_machine_sansheng.yaml) - 三省六部完整配置
- [state_machine_simple.yaml](examples/state_machine_simple.yaml) - 简单工作流配置
- [state_machine_software_dev.yaml](examples/state_machine_software_dev.yaml) - 软件开发工作流

### 示例 1：最简开发模式

仅启用状态机，使用默认三省六部配置：

```yaml
providers:
  openrouter:
    api_key: sk-or-v1-xxx

agents:
  defaults:
    model: anthropic/claude-sonnet-4-20250514

state_machine:
  enabled: true
```

### 示例 2：自定义状态机拓扑

定义简单的分析 → 执行 → 审核流程：

```yaml
state_machine:
  enabled: true
  config_path: "~/.nanobot/state_machine.yaml"
```

```yaml
# ~/.nanobot/state_machine.yaml
initial_state: analysis
terminal_states: [done]
active_states: [analysis, executing]
sync_roles: [analyst, reviewer]

transitions:
  - from: pending
    to: analysis
  - from: analysis
    to: executing
  - from: executing
    to: review
  - from: review
    to: done
  - from: review
    to: analysis  # 拒绝打回

state_roles:
  analysis: analyst
  executing: executor
  review: reviewer
  done: system
  pending: system

max_reviews: 3
stall_timeout_secs: 120
```

### 示例 3：长时间运行任务

适合需要更长超时的工作流：

```yaml
state_machine:
  enabled: true
  use_default_template: true
  max_reviews: 5
  stall_timeout_secs: 300  # 5 分钟超时
```

### 示例 4：禁用默认模板 + 完全自定义

```yaml
state_machine:
  enabled: true
  use_default_template: false
  config_path: "~/.nanobot/custom_workflow.yaml"
```

```yaml
# ~/.nanobot/custom_workflow.yaml
initial_state: intake
terminal_states: [completed, cancelled]
active_states: [intake, analysis, development, testing]
sync_roles: [intake_specialist, architect]

transitions:
  - from: pending
    to: intake
  - from: intake
    to: analysis
  - from: intake
    to: cancelled
  - from: analysis
    to: development
  - from: analysis
    to: cancelled
  - from: development
    to: testing
  - from: testing
    to: completed
  - from: testing
    to: development  # 打回修复

state_roles:
  intake: intake_specialist
  analysis: architect
  development: developer
  testing: qa_engineer
  completed: system
  cancelled: system
  pending: system

gates:
  testing:
    reject_to: development

max_reviews: 5
stall_timeout_secs: 180
```

---

## 编程 API

### Rust 代码中使用

```rust
use nanobot_statemachine::{
    bootstrap,
    StateMachineBootstrapConfig,
    StateMachineConfigBuilder,
};

// 方式 1: 使用 Builder 编程式创建配置
let config = StateMachineConfigBuilder::new()
    .initial_state("analysis")
    .add_terminal_state("done")
    .add_active_state("executing")
    .add_sync_role("analyst")
    .transition("pending", "analysis")
    .transition("analysis", "executing")
    .transition("executing", "done")
    .state_role("analysis", "analyst")
    .state_role("executing", "executor")
    .max_reviews(3)
    .build()?;

// 方式 2: 从文件加载
let config = nanobot_statemachine::load_from_yaml(
    std::path::Path::new("~/.nanobot/state_machine.yaml")
)?;

// 启动状态机子系统
let handle = bootstrap(
    &bootstrap_config,
    memory_store.pool().clone(),
    subagent_manager,
    &mut tool_registry,
).await?;
```

---

## 故障排查

### 任务一直卡在某个状态

1. 检查日志中是否有 `Stall detected` 信息
2. 使用 `state_machine_task` 工具的 `flow_log` 动作查看流转历史
3. 确认 `stall_timeout_secs` 设置是否合理（执行时间长的任务需要更大值）
4. 检查 Agent 是否在调用 `report_progress` 更新心跳

### 状态转换失败

1. 确认转换在 `transitions` 配置中定义
2. 检查当前状态是否允许转换到目标状态
3. 查看错误消息中返回的合法转换列表

### 停滞检测不工作

1. 确认状态在 `active_states` 中定义
2. 检查 `stall_timeout_secs` 是否设置合理
3. 查看日志确认 StallDetector 是否启动

---

## 从 Pipeline 迁移

如果你之前使用的是 `nanobot-pipeline` 模块，以下是迁移指南：

### 配置变更

```yaml
# 旧配置
pipeline:
  enabled: true

# 新配置
state_machine:
  enabled: true
  use_default_template: true  # 使用默认三省六部预设
```

### 工具名称变更

| 旧工具 | 新工具 |
|--------|--------|
| `pipeline_task` | `state_machine_task` |
| `delegate` | 已移除（通过状态转换实现） |
| `report_progress` | `report_progress` (不变) |

### API 变更

旧的委派模式：
```json
{
  "caller_role": "zhongshu",
  "target_role": "menxia",
  "task_description": "..."
}
```

新的状态转换模式：
```json
{
  "action": "transition",
  "task_id": "...",
  "to_state": "reviewing",
  "agent_role": "zhongshu",
  "reason": "规划完成，提交审核"
}
```

---

## 向后兼容性

| 场景 | 行为 |
|------|------|
| 配置中无 `state_machine` 字段 | 等同 `enabled: false`，零影响 |
| `state_machine.enabled: false` | 不创建表、不注册工具、不启动检测器 |
| 现有 `submit()` API | 完全不变 |
| 现有测试 | 全部通过 |

---

## 与 Pipeline 模块的对比

| 特性 | Pipeline (旧) | State Machine (新) |
|------|---------------|-------------------|
| 配置方式 | 硬编码 + 部分配置 | 完全数据驱动 |
| 状态定义 | TaskState 枚举 | YAML/JSON 配置 |
| 转换验证 | 硬编码逻辑 | 配置驱动验证 |
| 模块位置 | `nanobot-pipeline` | `nanobot-statemachine` |
| 工具名称 | `pipeline_task` | `state_machine_task` |
| 默认预设 | 仅三省六部 | 支持任意拓扑 |
| 代码复用 | 低 | 高（Builder 模式） |

`★ Insight ─────────────────────────────────────`
- State Machine 采用事件驱动架构，所有状态转换通过 `StateMachineEvent` 通道异步处理
- 配置验证在加载时执行，确保状态机拓扑的完整性和一致性
- Builder 模式 (`StateMachineConfigBuilder`) 支持编程式创建配置，无需编写 YAML 文件
`─────────────────────────────────────────────────`
