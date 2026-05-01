# PhaseTemplate 设计

> 在 `2026-05-01-phase-definition-design.md` 的 Step 1+2 基础上，引入 PhaseTemplate
> 作为 phase 转换的最外层过滤器（Layer 0），让用户通过 CLI 命令显式选择"流程形状"。

## 动机

Step 1+2 完成后，PhaseDefinition 已能逐 phase 表达入口提示、工具白名单、转换规则、硬门禁与退出 checklist。但目前所有任务都走"完整管线"的同一形状——LLM 自行决定是否跳过 Planning 或 Research。这带来两个问题：

1. **缺少形状的显式表达** — 用户清楚自己只想"快速搜一下"或"直接执行已知步骤"时，无法通过 CLI 把这种意图传给 LLM。
2. **LLM 浪费迭代探索不需要的 phase** — Research 阶段在简单任务里常常空跑几轮才决定推进。

PhaseTemplate 让用户**显式选择 5 种预定义流程形状**，引擎在 phase 转换时把模板未包含的目标统一拒绝在 Layer 0。无自动检测、无 LLM 分类、不抢决定权。

## 模板集合

| Template | 流程 | CLI 入口 |
|---|---|---|
| **FullFlow** | Research → Planning → Execute → Review | _(默认；无命令)_ |
| **PlanLed** | Planning → Execute → Review | `/plan` |
| **QuickExecute** | Research → Execute → Review | _(仅 config)_ |
| **DirectExecute** | Execute → Review | `/execute` |
| **ResearchOnly** | Research → Done | `/research` |

`Done` 是所有模板共通的终态，不进入 `allowed_phases` 的成员表。

## 选择优先级

```
ExecutorOptions::template            ← CLI / InboundMessage 显式指定
        ↓ (None)
KernelConfig.default_template        ← config.yaml
        ↓ (None)
PhaseTemplate::FullFlow              ← 硬编码 fallback
```

**不实现 `select_for_task()` 自动检测**——让选择权完全留给用户。

## 核心类型

### PhaseTemplate 枚举

```rust
// engine/src/kernel/phased/agent_phase.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PhaseTemplate {
    FullFlow,
    PlanLed,
    QuickExecute,
    DirectExecute,
    ResearchOnly,
}

impl PhaseTemplate {
    /// Phase the controller starts at when no resume is in effect.
    pub fn entry_phase(&self) -> AgentPhase;

    /// Phases reachable inside this template (Done excluded — universal terminal).
    pub fn allowed_phases(&self) -> &'static [AgentPhase];

    /// Layer 0 of the 4-layer transition filter.
    pub fn permits(&self, target: AgentPhase) -> bool {
        target == AgentPhase::Done || self.allowed_phases().contains(&target)
    }

    /// Linear next phase under hard-limit forced transition.
    /// Returns Done for the last phase and as defensive fallback for
    /// phases not in the template (theoretically unreachable).
    pub fn next_phase(&self, current: AgentPhase) -> AgentPhase;

    /// Snake-case serialization: full_flow / plan_led / quick_execute / direct_execute / research_only.
    pub fn as_str(&self) -> &'static str;
}

impl TryFrom<&str> for PhaseTemplate { /* … */ }
```

### `next_phase` 行为表

| Template | next(R) | next(P) | next(E) | next(Rv) |
|---|---|---|---|---|
| FullFlow | Planning | Execute | Review | Done |
| PlanLed | Done¹ | Execute | Review | Done |
| QuickExecute | Execute | Done¹ | Review | Done |
| DirectExecute | Done¹ | Done¹ | Review | Done |
| ResearchOnly | Done | Done¹ | Done¹ | Done¹ |

¹ phase 不在模板的 `allowed_phases` 中——理论不可达，但作为防御兜底返回 Done。

## 接口改造

### gasket-types

```rust
pub struct InboundMessage {
-    pub override_phase: Option<String>,
+    pub override_template: Option<String>,
    /* …其他字段不变 */
}

impl InboundMessage {
    /// Parse a slash command into a template string.
    /// Used by all 8 channel adapters; returns None for non-command input.
    pub fn parse_phase_command(input: &str) -> Option<String>;
}
```

`parse_phase_command` 的实现：

```rust
match input.trim() {
    "/plan" => Some("plan_led".into()),
    "/execute" => Some("direct_execute".into()),
    "/research" => Some("research_only".into()),
    _ => None,
}
```

严格小写——大小写不敏感会让命令边界模糊。

### engine

```rust
pub struct ExecutorOptions<'a> {
    pub vault_values: &'a [String],
-    pub start_phase: Option<&'a str>,
+    pub template: Option<&'a str>,
+    /// Resume mid-flow: phase to resume at. Must satisfy `template.permits()`.
+    pub resume_at: Option<&'a str>,
}

pub struct KernelConfig {
    /* 现有字段 */
+    pub default_template: PhaseTemplate,  // 默认 FullFlow
}
```

### PhaseController

```rust
impl PhaseController {
-    pub fn new(ctx: &RuntimeContext, start_phase: Option<AgentPhase>) -> Self
+    pub fn new(
+        ctx: &RuntimeContext,
+        template: PhaseTemplate,
+        resume_at: Option<AgentPhase>,
+    ) -> Self
}
```

构造时：
- `resume_at = None` → `current_phase = template.entry_phase()`
- `resume_at = Some(phase)` → 断言 `template.permits(phase)`，失败 panic（防御性 — 实际不可达）；`current_phase = phase`

### Channel 适配器

8 个 channel（CLI/DingTalk/Discord/Feishu/Slack/Telegram/WebSocket/WeChat/WeCom）当前各自有一份 `/plan` `/execute` `/research` → phase 字符串的解析。

改造：所有适配器替换为统一调用 `InboundMessage::parse_phase_command(text)`；解析结果写入 `override_template` 字段。每个文件预计 -3/+1 行。

## 转换决策（4 层过滤）

`post_step` 处理 `PhaseTransition` 的逻辑层级：

```
Layer 0: template.permits(target)            ← 新增
Layer 1: def.allowed_transitions.contains(target)
Layer 2: def.hard_gates 全部通过
Layer 3: def.exit_checklist 软检查（仅警告）
```

Layer 0 失败的拒绝消息：

```text
当前模板 {template} 不包含 {target} 阶段。允许的阶段：{allowed_phases.join(", ")}
```

注入回 LLM 作为 system message，让 LLM 在下一轮选合法目标。

### 强制转换（hard limit 兜底）

`pre_step` 在迭代达 hard limit 时调用 `force_transition`。改造：从 `PhaseDefinition.forced_transition_target` 读取改为 `template.next_phase(current)`。**`PhaseDefinition.forced_transition_target` 字段移除**——被模板吸收。

## 持久化

### Schema

phase 当前持久化在 `sessions_v2.current_phase`（commit 6985f1c 引入）。改造：

```sql
ALTER TABLE sessions_v2 ADD COLUMN current_template TEXT;
UPDATE sessions_v2 SET current_template = 'full_flow' WHERE current_template IS NULL;
```

回填 `'full_flow'` 是安全的——改造前不存在其他模板。`EventStore` 同步新增 `get_current_template()` / `set_current_template()`，与现有 `get_current_phase()` / `set_current_phase()` 配对。

### 读写规则

- 每次 phase 变更（init / transition / forced / resume）同步写入 template
- `PhaseController::new` 拿到的 template 为权威值；与持久化值不一致时新值覆盖（用户切换模板等于开新流程）

## 配置

```yaml
# ~/.gasket/config.yaml
default_template: full_flow  # 可选，缺省 FullFlow
```

非法字符串 → **fail-fast 启动报错**，列出合法值。运行时降级会掩盖部署错误。

## 测试策略

### 单元测试

| 模块 | 用例 |
|---|---|
| `PhaseTemplate` enum | `entry_phase` / `allowed_phases` / `permits`（含 Done 普适）/ `next_phase` 20 个组合 / `try_from` round-trip |
| `post_step` Layer 0 | ResearchOnly 拒绝 Planning；FullFlow 不被层 0 拒绝；拒绝消息含模板名与允许阶段 |
| `pre_step` forced | PlanLed+P 强制 → Execute；ResearchOnly+R 强制 → Done；DirectExecute+E 强制 → Review |
| Selection priority | explicit > config > FullFlow，三种组合 |
| `parse_phase_command` | 3 正例 / 1 非命令 / 1 大小写不敏感拒绝 |
| 持久化 round-trip | 写 phase+template，读出值一致 |

为支持单元测试 Layer 0 而不构造 `RuntimeContext`，把决策抽成纯函数 `evaluate_template_layer(template, target) -> Result<(), String>`。

### 集成测试（engine `tests/`）

5 个模板各一条端到端契约：用伪 LLM provider 跑 `KernelExecutor`，断言：
- 起始 phase 等于 `template.entry_phase()`
- LLM 触发 phase_transition 时引擎按模板推进
- 越界目标被拒绝且不改变状态
- 流程到达 Done

5 × ~30 行 ≈ 150 行。这是 Step 3 测试预算的大头。

### 不写的测试

- 8 个 channel 各自的命令解析（统一调 `InboundMessage::parse_phase_command`，单元测一遍即可）
- KernelConfig 的 YAML serde 解析（serde 自身已被充分测试）

## 影响范围

| 文件 | 变更 |
|---|---|
| `engine/src/kernel/phased/agent_phase.rs` | 新增 `PhaseTemplate` 枚举与方法；移除 `PhaseDefinition.forced_transition_target` |
| `engine/src/kernel/phased/phase_controller.rs` | `new()` 签名改造；`post_step` 加 Layer 0；`pre_step` forced 改用 `template.next_phase` |
| `engine/src/kernel/kernel_executor.rs` | `ExecutorOptions` 字段改名；selection priority 落实 |
| `engine/src/kernel/context.rs` | `KernelConfig.default_template` 字段 |
| `gasket-types` | `InboundMessage.override_template`；`parse_phase_command` helper |
| `engine/src/channels/*` (×8) | 替换为统一 helper 调用 |
| `engine/src/storage/*` (`sessions_v2` 表与 `EventStore`) | 加 `current_template` 列 + 回填 migration；新增 `get/set_current_template` |
| `engine/tests/` | 新增 5 个模板的集成测试 |

## 迁移路径

三步，每步独立可编译可测：

### Step 3a — 类型与纯函数

- 引入 `PhaseTemplate` 枚举与所有方法（`entry_phase` / `allowed_phases` / `permits` / `next_phase` / `as_str` / `try_from`）
- 抽出 `evaluate_template_layer` 纯函数
- 抽出 `InboundMessage::parse_phase_command` helper
- **验证：** 上述类型的单元测试全部通过；现有测试不退化；`PhaseDefinition.forced_transition_target` 仍存在但不被引用

### Step 3b — 接口与控制流改造

- `ExecutorOptions` / `InboundMessage` / `PhaseController::new` 签名改造
- `post_step` 加入 Layer 0；`pre_step` forced 改用 `template.next_phase`
- 移除 `PhaseDefinition.forced_transition_target` 字段及所有引用
- 8 个 channel 适配器统一改用 `parse_phase_command` helper
- `KernelConfig.default_template` 字段
- **验证：** workspace 编译通过；新增 Layer 0 与 forced 单元测试通过；现有测试不退化

### Step 3c — 持久化与集成测试

- SQLite migration 加 `template` 列
- PhaseController 读写 template 与 phase 同步
- 5 个模板的集成测试
- **验证：** 集成测试 5 个全部绿；`cargo test --workspace` 全过

## 不做的事（YAGNI）

- `select_for_task()` 自动检测
- LLM 分类调用
- 模板的可扩展注册机制（5 个枚举值够用；新增需求再说）
- 模板与 phase 不一致时的运行时降级（fail-fast 优于隐式回退）
- 大小写不敏感的命令解析
