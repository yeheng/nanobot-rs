# 多 Agent 协作管线 (三省六部)

> Gasket 多 Agent 分层协作框架使用指南

---

## 概述

Pipeline 子系统为 gasket 引入了**分层多 Agent 协作机制**，灵感来自中国古代的[「三省六部」](https://github.com/cft0808/edict)治理体系。它提供：

- **任务状态机** — 严格的生命周期管理，杜绝非法状态跳转
- **权限矩阵** — 有向图授权，Agent 只能委派到允许的目标角色
- **审核门控** — 门下省 (Menxia) 作为强制质量关卡，可拒绝打回
- **停滞检测** — 自动发现超时任务并触发恢复策略
- **完全 opt-in** — `pipeline.enabled: false`（默认）时零开销，不创建表，不注册工具

---

## 架构

```
用户请求
    ↓
[OrchestratorActor] ← PipelineEvent (mpsc channel)
    ↓ dispatch
[Triage] → [Planning] ⇄ [Review] → [Dispatch] → [Ministry₁..₆]
   太子       中书省      门下省       尚书省       六部执行层
    ↓                                                  ↓
  分析分类                                         执行 + 进度上报
```

### 三层架构

| 层次 | 角色 | 职责 |
|------|------|------|
| **分诊层** | 太子 (taizi) | 分析请求、分类优先级 |
| **治理层** | 中书省 (zhongshu) | 战略规划、任务分解 |
| | 门下省 (menxia) | 审核质量门控，可拒绝打回 |
| | 尚书省 (shangshu) | 任务调度分发 |
| **执行层** | 礼部 (li) | 文档、协议、通信 |
| | 户部 (hu) | 数据管理、资源分析 |
| | 兵部 (bing) | 运维、部署、基础设施 |
| | 刑部 (xing) | 合规、安全审计 |
| | 工部 (gong) | 开发、工程实现 |
| | 殿中 (dianzhong) | 人事、协调、行政 |

### 调度模式

- **治理层 Agent**（taizi、zhongshu、menxia、shangshu）使用 `submit_and_wait()` — 同步等待决策结果
- **执行层 Agent**（六部）使用 `submit()` — 异步 fire-and-forget，通过 `report_progress` 工具上报进度

---

## 快速开始

### 1. 启用 Pipeline

在 `~/.gasket/config.yaml` 中添加：

```yaml
pipeline:
  enabled: true
```

这会使用所有默认值启动管线：内置三省六部角色、默认权限矩阵、默认 SOUL 模板。

### 2. 验证启动

启动 gasket 后，日志中应出现：

```
Pipeline orchestrator started
Pipeline stall detector started (timeout=60s, interval=30s)
```

Agent 工具列表中会新增三个工具：
- `pipeline_task` — 任务看板
- `delegate` — 权限委派
- `report_progress` — 进度上报

---

## 配置参考

### 最小配置

```yaml
pipeline:
  enabled: true
```

### 完整配置

```yaml
pipeline:
  # 主开关 — false 或缺省时整个子系统完全休眠
  enabled: true

  # 加载内置三省六部默认角色模板（默认 true）
  useDefaultTemplate: true

  # 审核往返最大次数，超过则升级处理（默认 3）
  maxReviews: 3

  # 心跳超时秒数，超过视为停滞（默认 60）
  stallTimeoutSecs: 60

  # 全局模型覆盖 — 所有管线 Agent 使用此模型
  model: anthropic/claude-sonnet-4-20250514

  # 角色定义（合并到默认模板之上）
  roles:
    zhongshu:
      description: "中书省 - 战略规划"
      allowedAgents: ["menxia"]
      model: "anthropic/claude-opus-4-20250514"  # 规划用更强模型

    menxia:
      description: "门下省 - 审核质量门控"
      allowedAgents: ["shangshu", "zhongshu"]

    shangshu:
      description: "尚书省 - 任务调度"
      allowedAgents: ["li", "hu", "bing", "xing", "gong", "dianzhong"]

    gong:
      description: "工部 - 开发实现"
      allowedAgents: ["shangshu"]
      soulPath: "/path/to/custom/gong_soul.md"  # 自定义 SOUL 模板
```

### 配置字段说明

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | bool | `false` | 主开关 |
| `useDefaultTemplate` | bool | `true` | 是否加载内置三省六部模板 |
| `maxReviews` | u32 | `3` | 审核最大轮次 |
| `stallTimeoutSecs` | u64 | `60` | 心跳超时阈值（秒） |
| `model` | string? | `null` | 全局模型覆盖 |
| `roles` | map | `{}` | 角色定义映射 |

### 角色定义字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `description` | string | 角色描述 |
| `allowedAgents` | string[] | 允许委派的目标角色列表 |
| `soulPath` | string? | 自定义 SOUL.md 路径 |
| `model` | string? | 此角色专用模型覆盖 |
| `responsibleStates` | string[] | 负责的任务状态 |

---

## 任务状态机

### 状态流转图

```
Pending → Triage → Planning → Reviewing ──→ Assigned → Executing → Review → Done
                      ↑          │                         │          │
                      └──────────┘                         │          │
                      (拒绝打回)                            ↓          ↓
                                                       Blocked ← ← ←
                                                         │
                                    ┌────────────────────┤
                                    ↓                    ↓
                                 Executing            Planning
                                 (恢复执行)           (恢复规划)
```

### 状态说明

| 状态 | 负责角色 | 说明 |
|------|---------|------|
| `pending` | system | 新创建，等待进入管线 |
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

## 权限矩阵

默认权限遵循严格的层级委派：

```
taizi    → [zhongshu]                               # 分诊 → 规划
zhongshu → [menxia]                                 # 规划 → 审核
menxia   → [shangshu, zhongshu]                     # 审核 → 调度 或 打回规划
shangshu → [li, hu, bing, xing, gong, dianzhong]    # 调度 → 六部
六部     → [shangshu]                               # 六部只能回报调度
```

**关键约束**：
- 不可跨层委派（太子不能直接调度六部）
- 六部之间不可互相委派
- 只有门下省可以"向上"拒绝打回中书省

可通过 `roles.*.allowedAgents` 配置扩展默认权限。

---

## 工具使用指南

### pipeline_task — 任务看板

任务看板是 Agent 与共享任务板交互的唯一接口。

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

#### 查询任务

```json
{
  "action": "get",
  "task_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

#### 列出任务

```json
{
  "action": "list",
  "state": "executing"
}
```

```json
{
  "action": "list",
  "role": "gong"
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

#### 查询流转日志

```json
{
  "action": "flow_log",
  "task_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

### delegate — 权限委派

Agent 间的核心通信机制。调用前自动校验权限矩阵。

#### 同步委派（等待结果）

```json
{
  "caller_role": "zhongshu",
  "target_role": "menxia",
  "task_description": "请审核以下执行计划：...",
  "sync": true
}
```

#### 异步委派（fire-and-forget）

```json
{
  "caller_role": "shangshu",
  "target_role": "gong",
  "task_description": "请执行以下开发任务：...",
  "sync": false
}
```

#### 权限拒绝示例

```json
{
  "caller_role": "gong",
  "target_role": "li",
  "task_description": "..."
}
// → 错误: "Role 'gong' is not allowed to delegate to 'li'"
```

### report_progress — 进度上报

执行层 Agent 定期调用，同时更新心跳时间戳。

```json
{
  "task_id": "550e8400-e29b-41d4-a716-446655440000",
  "agent_role": "gong",
  "content": "已完成数据库 schema 设计，正在实现 API 层",
  "percentage": 60.0
}
```

**重要**：执行层 Agent 必须每 30 秒内至少上报一次进度，否则会被停滞检测标记。

---

## 停滞检测与恢复

### 检测机制

`StallDetector` 每 30 秒扫描一次活跃任务（状态为 executing、triage、planning、reviewing、assigned），检查 `last_heartbeat` 是否超过 `stallTimeoutSecs`。

### 三级恢复策略

| 级别 | 条件 | 动作 |
|------|------|------|
| L1 重试 | 首次停滞 (retry_count=0) | 重新调度同一 Agent |
| L2 阻塞 | 重试后仍停滞 (retry_count≥1) | 转为 Blocked 状态 |
| 手动恢复 | Blocked 状态 | 管理员通过 pipeline_task 工具恢复到 Executing 或 Planning |

---

## SQLite 表结构

Pipeline 使用三张独立表，与现有 session/kv 表共享同一数据库：

### pipeline_tasks

```sql
CREATE TABLE pipeline_tasks (
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
    origin_chat_id  TEXT                    -- 来源会话
);
```

### pipeline_flow_log

```sql
CREATE TABLE pipeline_flow_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id     TEXT NOT NULL,
    from_state  TEXT NOT NULL,
    to_state    TEXT NOT NULL,
    agent_role  TEXT NOT NULL,
    reason      TEXT,
    timestamp   TEXT NOT NULL
);
```

### pipeline_progress_log

```sql
CREATE TABLE pipeline_progress_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id     TEXT NOT NULL,
    agent_role  TEXT NOT NULL,
    content     TEXT NOT NULL,
    percentage  REAL,
    timestamp   TEXT NOT NULL
);
```

**乐观锁**：`update_task_state()` 使用 `WHERE state = :expected` 防止并发冲突。

---

## SOUL 模板

每个角色有一个 SOUL.md 文件定义其身份、工具权限、工作流和约束。默认模板位于 `workspace/pipeline_templates/`：

| 文件 | 角色 |
|------|------|
| `taizi.md` | 太子 — 分诊 |
| `zhongshu.md` | 中书省 — 规划 |
| `menxia.md` | 门下省 — 审核 |
| `shangshu.md` | 尚书省 — 调度 |
| `ministry_default.md` | 六部通用执行 |

### 自定义 SOUL

通过 `roles.*.soulPath` 指定自定义模板：

```yaml
pipeline:
  enabled: true
  roles:
    gong:
      description: "工部 - 专注 Rust 开发"
      soulPath: "/home/user/.gasket/souls/gong_rust.md"
```

---

## 模块结构

```
gasket-core/src/pipeline/
├── mod.rs              模块入口 + re-exports
├── config.rs           PipelineConfig, AgentRoleDef
├── state_machine.rs    TaskState 枚举, 转换验证
├── models.rs           PipelineTask, FlowLogEntry, ProgressEntry
├── store.rs            SQLite 持久化层 (PipelineStore)
├── orchestrator.rs     OrchestratorActor 事件循环
├── permission.rs       PermissionMatrix 权限矩阵
└── stall_detector.rs   StallDetector 停滞检测

gasket-core/src/tools/
├── pipeline_task.rs    任务看板工具
├── delegate.rs         权限委派工具
└── report_progress.rs  进度上报工具
```

---

## 配置示例合集

### 示例 1：最简开发模式

仅启用管线，全部使用默认配置：

```yaml
providers:
  openrouter:
    api_key: sk-or-v1-xxx

agents:
  defaults:
    model: anthropic/claude-sonnet-4-20250514

pipeline:
  enabled: true
```

### 示例 2：规划用强模型 + 执行用快模型

```yaml
providers:
  openrouter:
    api_key: sk-or-v1-xxx

agents:
  defaults:
    model: anthropic/claude-sonnet-4-20250514

pipeline:
  enabled: true
  model: anthropic/claude-sonnet-4-20250514   # 默认模型
  roles:
    zhongshu:
      description: "中书省 - 使用 Opus 进行战略规划"
      allowedAgents: ["menxia"]
      model: "anthropic/claude-opus-4-20250514"
    menxia:
      description: "门下省 - 使用 Opus 进行质量审核"
      allowedAgents: ["shangshu", "zhongshu"]
      model: "anthropic/claude-opus-4-20250514"
```

### 示例 3：自定义角色 + 宽松超时

适合长时间运行的任务：

```yaml
pipeline:
  enabled: true
  maxReviews: 5
  stallTimeoutSecs: 300        # 5 分钟超时
  roles:
    gong:
      description: "工部 - 全栈开发"
      allowedAgents: ["shangshu"]
      soulPath: "~/.gasket/souls/fullstack_dev.md"
    bing:
      description: "兵部 - DevOps"
      allowedAgents: ["shangshu"]
      soulPath: "~/.gasket/souls/devops.md"
```

### 示例 4：禁用默认模板 + 完全自定义

```yaml
pipeline:
  enabled: true
  useDefaultTemplate: false
  roles:
    analyst:
      description: "需求分析师"
      allowedAgents: ["architect"]
      responsibleStates: ["triage"]
    architect:
      description: "架构师"
      allowedAgents: ["reviewer"]
      responsibleStates: ["planning"]
    reviewer:
      description: "代码审核"
      allowedAgents: ["developer", "architect"]
      responsibleStates: ["reviewing", "review"]
    developer:
      description: "开发工程师"
      allowedAgents: ["reviewer"]
      responsibleStates: ["executing"]
```

---

## 向后兼容性

| 场景 | 行为 |
|------|------|
| 配置中无 `pipeline` 字段 | 等同 `enabled: false`，零影响 |
| `pipeline.enabled: false` | 不创建表、不注册工具、不启动检测器 |
| 现有 `submit()` API | 完全不变 |
| 现有配置解析 | `pipeline` 字段是 `Option<PipelineConfig>`，缺省 = `None` |
| 现有测试 | 全部通过（11 个 config 测试 + 其他模块测试） |

---

## 故障排查

### 任务一直卡在某个状态

1. 检查日志中是否有 `Stall detected` 信息
2. 使用 `pipeline_task` 工具的 `flow_log` 动作查看流转历史
3. 确认 `stallTimeoutSecs` 设置是否合理（执行时间长的任务需要更大值）
4. 检查 Agent 是否在调用 `report_progress` 更新心跳

### 权限拒绝错误

1. 确认 `roles.*.allowedAgents` 配置是否包含目标角色
2. 检查默认权限矩阵是否覆盖了预期路径
3. 使用 `useDefaultTemplate: true` 确保基础权限已加载

### 审核超限

1. 检查 `maxReviews` 配置（默认 3）
2. 查看 flow_log 确认是否存在 reviewing ↔ planning 循环
3. 适当增大 `maxReviews` 或优化规划质量
