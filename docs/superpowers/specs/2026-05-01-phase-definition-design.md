# PhaseDefinition 重构设计

> 借鉴 superpowers skill 模式，将 AgentPhase 从"枚举 + 散布式 match"重构为"自包含的声明式阶段定义"。

## 动机

当前 AgentPhase 的规则分散在 4 个独立方法（`allowed_tools()`, `can_transition_to()`, `max_iterations()`, `soft_limit_iterations()`）和 `PhasePrompt::entry_prompt()` 的 match 分支中。这带来三个问题：

1. **缺少阶段门禁** — LLM 可以在 Research 未收集任何信息时跳到 Planning，或在 Planning 未产出计划时进入 Execute。
2. **缺少结构化产出契约** — 各阶段只靠 prompt 引导输出，没有 checklist 定义"合格产出应该包含什么"。
3. **缺少可组合性** — 5 个 phase 是固定的线性流程，简单任务也必须走完整管线。

## 核心类型

### ChecklistItem

```rust
struct ChecklistItem {
    label: String,
    auto_verify: Option<fn(&PhaseContext) -> bool>,
}
```

单条退出检查项。`auto_verify` 为 `fn` 指针（轻量同步检查），`None` 表示靠 prompt 引导 LLM 自查。

### GateCheck

```rust
trait GateCheck: Send + Sync {
    fn description(&self) -> &str;
    fn check(&self, ctx: &PhaseContext) -> GateResult;
}

enum GateResult {
    Passed,
    Failed(String),
}
```

引擎强制的硬门禁接口。返回 `Failed` 时附带原因，注入回 LLM prompt。

### PhaseContext

```rust
struct PhaseContext {
    tools_invoked: Vec<String>,
    files_written: Vec<String>,
    wiki_pages_written: Vec<String>,
    iterations: usize,
    context: ContextAccumulator,
}
```

阶段执行期间的运行时快照，供 gate/checklist 查询。由 `PhaseController` 在每次 `post_step` 增量构建。

### PhaseDefinition

```rust
struct PhaseDefinition {
    phase: AgentPhase,
    entry_prompt: fn(&ContextAccumulator) -> String,
    allowed_tools: fn() -> Vec<&'static str>,
    exit_checklist: Vec<ChecklistItem>,
    hard_gates: Vec<Box<dyn GateCheck>>,
    soft_gates: Vec<String>,
    max_iterations: usize,
    soft_limit_iterations: usize,
    allowed_transitions: Vec<AgentPhase>,
    forced_transition_target: Option<AgentPhase>,
}
```

自包含的阶段定义。将当前散布在枚举方法和 match 分支中的所有规则合并为一个内聚单元。

### PhaseControl

```rust
enum PhaseControl {
    Continue,
    Transition,
    Interrupt,
    Reject(String),
    RejectWithGuidance(String),
}
```

新增 `Reject` 变体用于门禁拒绝，`RejectWithGuidance` 附带原因供 LLM 修正。

## PhaseTemplate — 预定义流程模板

```rust
enum PhaseTemplate {
    FullFlow,       // Research → Planning → Execute → Review
    QuickExecute,   // Research → Execute → Review
    ResearchOnly,   // Research → Done
    DirectExecute,  // Execute → Review
}
```

模板提供两层过滤的交集约束：
- 模板限定"哪些阶段存在"
- `PhaseDefinition.allowed_transitions` 限定"当前阶段可以去哪"

模板选择策略：
- CLI `/plan`、`/execute` 等命令隐含模板切换
- `PhaseController::initialize` 根据用户消息自动选择
- `config.yaml` 中可选 `default_template` 覆盖自动检测

## PhaseController 重构

`PhaseController` 从"硬编码 match 策略"变为"查表驱动的执行引擎"：

```
struct PhaseController {
    template: PhaseTemplate,
    definitions: HashMap<AgentPhase, PhaseDefinition>,
    state_machine: PhaseStateMachine,
    context: ContextAccumulator,
    phase_trace: PhaseContext,
}
```

### initialize

1. 选择模板（自动检测或用户覆盖）
2. 进入模板的第一个阶段
3. 从 `PhaseDefinition` 生成 entry prompt
4. 注入退出 checklist（软检查项）

### post_step — 四层过滤

transition 请求经过四层检查：

1. **模板层** — `template.is_transition_allowed(current, target)`
2. **阶段定义层** — `def.allowed_transitions.contains(target)`
3. **硬门禁层** — 所有 `hard_gates` 必须通过
4. **软检查层** — `auto_verify` 可验证的自动检查，不可验证的记录警告但不阻断

硬门禁失败返回 `RejectWithGuidance`，`PhaseTransitionTool` 将其作为 tool result 返回 LLM。

### pre_step

逻辑不变，配置从 `definitions[&phase]` 读取而非枚举方法调用。

## 各阶段定义要点

| Phase   | Hard Gate          | Exit Checklist              | Allowed Transitions       |
|---------|--------------------|-----------------------------|---------------------------|
| Research | (none)            | ☐ 信息已充分收集             | Planning, Execute         |
|         |                    | ☐ 已向用户总结发现           |                           |
| Planning | files_written 非空 | ☐ 计划包含步骤列表           | Execute, Research         |
|         |                    | ☐ 步骤含验证标准             |                           |
| Execute  | (none)            | ☐ 计划步骤已执行             | Review, Planning          |
|         |                    | ☐ 产出物已验证               |                           |
| Review   | (none)            | ☐ 结果达成目标               | Planning, Execute, Done   |
|         |                    | ☐ 知识已持久化               |                           |
| Done     | (terminal)        | (none)                      | (none)                    |

仅 Planning 有硬门禁（必须产出文件）。其他阶段靠 prompt + 软检查。YAGNI——先在最易验证的点上加固。

## 迁移路径

三步，每步独立可编译可测试：

### Step 1: 引入类型，并行运行

- 新增 `PhaseDefinition`, `ChecklistItem`, `GateCheck`, `PhaseContext` 类型
- 新增 `fn default_definitions() -> HashMap<AgentPhase, PhaseDefinition>` 返回基于当前逻辑的定义实例
- `PhaseController` 新增 `definitions` 字段，暂不使用
- **验证：** 编译通过 + 现有测试不退化

### Step 2: 切换到查表执行

- `post_step` transition 逻辑从 match 切换为查表
- `pre_step` 从枚举方法切换为 `definitions[&phase]` 读取
- `entry_prompt` 从 `PhasePrompt` 切换为 `definitions[&phase].entry_prompt`
- 移除 `AgentPhase` 上的 `allowed_tools()`, `can_transition_to()`, `max_iterations()`, `soft_limit_iterations()`, `forced_transition_target()` 方法
- **验证：** 现有测试通过 + 新增 gate/checklist 单元测试

### Step 3: 引入 PhaseTemplate

- 新增 `PhaseTemplate` 枚举和 `select_for_task()`
- `initialize` 使用模板选择起始阶段
- CLI 命令映射到模板选择
- **验证：** 集成测试覆盖每个模板的完整流程

## 影响范围

| 文件 | 变更类型 |
|------|---------|
| `engine/src/kernel/phased/agent_phase.rs` | 重大重构：枚举方法 → PhaseDefinition 结构体 |
| `engine/src/kernel/phased/phase_controller.rs` | 重大重构：match → 查表执行 |
| `engine/src/kernel/phased/phase_prompt.rs` | entry_prompt 移入 PhaseDefinition |
| `engine/src/tools/phase_transition.rs` | 适配 RejectWithGuidance 返回 |
| `engine/src/kernel/context.rs` | PhaseContext 集成 |
| `engine/src/kernel/kernel_executor.rs` | 模板初始化适配 |
| 其他文件 | 无变更 |
