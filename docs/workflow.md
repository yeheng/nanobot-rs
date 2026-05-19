# Workflow 使用指南

## 概述

Workflow 是一种**用 YAML 定义的多步骤 LLM 编排流程**。它把"研究 → 计划 → 实现 → 评审"这类有清晰阶段、有评审/重试节点的任务模式抽象出来，让你不必每次都用提示词重新拼。

Gasket 把 workflow 的"定义"和"执行"分成两层：

| 层 | 类型 | 职责 |
|---|---|---|
| Manifest | `WorkflowManifest` | YAML 直接反序列化得到的数据，仅做语法校验 |
| Workflow | `Workflow` | 经过结构性验证、索引化的执行图，运行时使用 |

这种分层让所有"步骤名是否存在""DONE 是否可达""字段拼错"等问题都在加载阶段就 fail-fast，运行时只是 dumb 地按索引执行。

## 文件位置与发现

workflow YAML 文件放在工作区的 `workflows/` 子目录中：

```
$GASKET_WORKSPACE/workflows/
├── dev.yaml
├── self-evolution.yaml
└── *.yaml | *.yml
```

启动时 Gasket 会扫描该目录，根据每个文件的 `mode` 字段把它分发到两个注册表之一：

```
mode: tool   →  作为工具注册到 ToolRegistry（LLM 调用工具执行）
mode: skill  →  作为技能注入到 system prompt（LLM 自主按步骤执行）
```

默认 `mode: tool`。两个目录共用同一个 YAML 文件夹。

## YAML 字段参考

```yaml
name: "my_workflow"           # 工具名 / 技能名（在 LLM 视角下唯一）
description: "..."             # 工具描述，LLM 用来判断何时调用
mode: tool                     # tool | skill，默认 tool
always: true                   # 仅 skill 模式生效，默认 true，是否常驻 system prompt
parameters:                    # JSON Schema 参数定义（与工具参数一致）
  type: object
  properties:
    task:
      type: string
      description: "用户要做的事"
  required: ["task"]
output_template: |             # 可选；最终输出渲染模板
  ## Result
  {{step_name}}
start_step: "first"            # 起始步骤名
steps:
  first:
    prompt: "Do {{input.task}}"
    model: "claude-sonnet-4-6" # 可选，覆盖此步骤的模型
    next: "second"             # 下一步骤名，或 "DONE" 终止
  second:
    prompt: "Review {{first}}"
    evaluate:                  # 与 next 互斥，按 verdict 分支
      on_pass: "DONE"
      on_fail: "first"
      max_retries: 3
```

### 字段约束

| 字段 | 类型 | 必填 | 默认 | 说明 |
|---|---|---|---|---|
| `name` | string | 是 | — | 不可为空 |
| `description` | string | 是 | — | 不可为空 |
| `parameters` | JSON Schema | 是 | — | 标准 JSON Schema |
| `start_step` | string | 是 | — | 必须存在于 `steps` 中 |
| `steps` | map | 是 | — | 至少一个步骤 |
| `mode` | enum | 否 | `tool` | 仅 `tool` / `skill`，拼错（如 `skil`）直接报错 |
| `always` | bool | 否 | `true` | 仅 skill 模式有意义 |
| `output_template` | string | 否 | — | 不设则返回 `{"context": {...}}` JSON |

**严格 schema**：上面三个结构（`WorkflowManifest`、`WorkflowStepDef`、`EvaluateConfigDef`）均开启 `deny_unknown_fields`。任何未知字段（含已废弃的 `condition`）会让加载失败。这是有意为之——拼写错误必须早 fail。

### 步骤定义

每个步骤至少有 `next` 或 `evaluate` 之一：

```yaml
steps:
  step_a:
    prompt: "..."
    next: "step_b"           # 无条件跳转
  step_b:
    prompt: "..."
    evaluate:                # 条件跳转，依赖输出中的 verdict
      on_pass: "step_c"
      on_fail: "step_a"
      max_retries: 3
```

- 跳转目标必须是 `steps` 中的某个名字，或字面值 `"DONE"` 表示终止
- 所有步骤都必须从 `start_step` 出发可达（顺着 `next` / `on_pass` / `on_fail` 三种边）。**孤儿步骤会让加载报错**
- 步骤数量上限 100（防御性的死循环兜底）

## 两种执行模式

### `mode: tool`（状态机执行）

LLM 把 workflow 当成一个普通工具调用。引擎接收到调用后：

1. 按 `start_step` 启动状态机
2. 每个步骤通过 `spawn_with_stream` 启动一个子 agent 执行 prompt
3. 子 agent 的输出存入 `context_map`，键为步骤名
4. 根据 `next` 或 `evaluate` 决定下一步
5. 到 `DONE` 后渲染 `output_template`（如未配置则返回 JSON）

子 agent 流式事件会实时转发到前端，UI 能看到每步的进度。

适用场景：

- 流程明确、需要严格按顺序执行
- 步骤之间需要不同模型（如评估用强模型、执行用快模型）
- 需要重试边界明确的评审节点

### `mode: skill`（提示词注入）

workflow 不作为工具注册，而是被转换成 markdown 注入到 LLM 的 system prompt 中。LLM 在普通对话循环中按文档描述自主执行，**没有子 agent，没有状态机**。

转换后的 markdown 结构：

```markdown
## Workflow: my_workflow

[description]

**Parameters**:
- `task`: 用户要做的事

**Execution Rules**:
1. Execute the following steps in order...
2. Context flows naturally through conversation history...
3. If a step is clearly unnecessary, skip it flexibly but inform the user.

### Execution Steps

#### 1. step_a
[prompt 内容，{{var}} 被替换成 "[see above]"]

#### 2. step_b
[prompt 内容]

*Review Rules*:
- On pass proceed to: **DONE**
- On fail return to: **step_a** (max retries: 3)

**End Rule**: After all steps are complete, output the final result...
```

适用场景：

- 流程是"约束/建议"而非"严格状态机"
- 希望复用 LLM 已有的对话上下文（不引入子 agent 开销）
- 步骤本身比较灵活，需要 LLM 根据情况跳过或调整

`always: true`（默认）让 skill 常驻 system prompt。如果你有很多 skill 工作流，要意识到 system prompt 体积会线性增长——这时改 `always: false`，并通过其他 skill 加载机制按需召唤。

## 模板与上下文

### 模板语法

`{{key}}` 占位符，key 允许的字符：`[a-zA-Z0-9_./]`。

```yaml
prompt: |
  Task: {{input.task}}
  Previous result: {{research}}
  Failure reason: {{review.reason}}
```

未知键保留原样（不会替换成空字符串），这样 LLM 能看到"哪个变量没填上"。

### 上下文键命名约定

| 键格式 | 来源 |
|---|---|
| `input.<param_name>` | 用户调用工具时传入的参数 |
| `<step_name>` | 该步骤子 agent 的最终输出 |
| `<step_name>.reason` | `evaluate` 步骤解析出的 reason（仅评审节点） |

例：dev.yaml 中 `{{review.reason}}` 在 `output_template` 里使用，表示评审步骤最后一次的失败/通过原因。

### output_template

如果配置了 `output_template`，workflow 完成后会渲染它作为最终输出；未配置则返回所有 context 的 JSON 序列化。

```yaml
output_template: |
  ## Result: PASS

  **Final review**: {{review.reason}}

  ### Generated code
  {{implement}}
```

## 评审节点：verdict / pass_gate

`evaluate` 步骤要求该步骤的 prompt 引导 LLM 输出**包含 verdict 字段的 JSON**：

```json
{
  "verdict": "PASS",
  "reason": "All checks passed."
}
```

解析规则（按优先级）：

1. **首选**：`verdict: "PASS" | "FAIL"`（大小写不敏感）
2. **兼容回退**：`pass_gate: true|false` 或 `validation_passed: true|false`（会打印 deprecation warning）
3. 都没有 → 报错 → 当作 FAIL 计入重试

`reason` 字段：

- 有 `reason` 字段（字符串）→ 直接使用
- 无 → 把整个 JSON 对象序列化成字符串作为 reason（保证 loop-back 步骤至少能拿到上下文）

### LLM 输出容错

实际 LLM 经常输出"JSON + 解释文字"或" \`\`\`json 围栏 + JSON"。引擎使用 `serde_json::Deserializer` 流式提取：

```
1. 找到首个 `{`
2. 从该位置流式反序列化，遇到匹配的 `}` 即停止
3. 后续文字一律忽略
```

所以以下都能解析：

```
{"verdict":"PASS","reason":"ok"}

Note: this run was clean.        ← 尾随文字，被丢弃
```

```
Here's my verdict:
{"verdict":"FAIL","reason":"x"}  ← 前置文字会让 find('{') 跳过
```

```
```json
{"verdict":"PASS","reason":"ok"}
```                              ← markdown 围栏，被丢弃
```

### 重试与退出

- `verdict: PASS` → 跳转 `on_pass`
- `verdict: FAIL` → 跳转 `on_fail`，且该节点的失败计数 `+1`
- 失败计数 > `max_retries` → 整个 workflow 报错退出
- 计数按"评审节点的 index"计，不按整条 workflow 累计

## 完整示例

参考工作区内置的两个 workflow：

**`workspace/workflows/dev.yaml`**（skill 模式）：研究 → 计划 → 实现 → 评审循环

```yaml
name: "dev_workflow"
description: "Research → Plan → Implement → Review loop for code generation"
mode: "skill"
output_template: |
  ## Dev Workflow Result: PASS
  - **Final Review**: {{review.reason}}
  ### Generated Code
  {{implement}}
parameters:
  type: object
  properties:
    task:
      type: string
      description: "What to build"
  required: ["task"]
start_step: "research"
steps:
  research:
    prompt: "Research context for: {{input.task}}"
    next: "plan"
  plan:
    prompt: "Plan the implementation. Research: {{research}}"
    next: "implement"
  implement:
    prompt: "Implement the plan: {{plan}}"
    next: "review"
  review:
    prompt: |
      Review the implementation: {{implement}}
      Output JSON: {"verdict":"PASS|FAIL", "reason":"..."}
    evaluate:
      on_pass: "DONE"
      on_fail: "implement"
      max_retries: 3
```

**`workspace/workflows/self-evolution.yaml`**（tool 模式）：execute → evaluate → diagnose → refine → validate → distill 闭环，演示了 on_fail 分支独立可达的拓扑。

## 最佳实践

### 1. 步骤要"原子"

一个步骤做一件能被独立评审的事。"研究 + 规划"放在一起，评审节点就没法判断"是研究不充分还是规划错了"。

不好：

```yaml
prepare:
  prompt: "Research and plan: {{input.task}}"
  next: "implement"
```

好：

```yaml
research:
  prompt: "Research context for: {{input.task}}"
  next: "plan"
plan:
  prompt: "Plan based on research: {{research}}"
  next: "implement"
```

### 2. 评审节点必须在 prompt 中明确 JSON 格式

不要假设 LLM 知道你期待什么 schema。把目标 JSON 整段写进 prompt：

```yaml
review:
  prompt: |
    Review the code: {{implement}}
    
    Output ONLY a JSON object in this exact format:
    {"verdict": "PASS" | "FAIL", "reason": "<one sentence>"}
```

引擎对 LLM 输出有容错（见上文），但**清晰的 prompt 比依赖容错重要得多**——它能减少重试次数，进而省 token。

### 3. `max_retries` 不要超过 3

每次重试是一整个子 agent 调用。重试 5 次意味着同一步骤最多跑 6 遍。如果默认 3 次还过不了，说明：

- prompt 有问题（最常见）
- 任务本身超出当前模型能力
- 评审标准过苛

应该改 prompt 或评审，而不是加 max_retries。

### 4. tool 模式按 step 选模型

`evaluate` 类节点用强模型（Opus/Sonnet）以保证评估严格；`execute`/`refine` 类节点可以用更快的模型。例：

```yaml
execute:
  prompt: "..."
  model: "claude-sonnet-4-6"
  next: "evaluate"
evaluate:
  prompt: "..."
  model: "claude-opus-4-7"     # 评审用强模型
  evaluate: { ... }
```

模型字段省略则用引擎默认。

### 5. skill 模式慎用 `always: true`

skill workflow 常驻 system prompt，每次对话都被注入。如果你有 5 个 skill workflow 都 `always: true`，每次对话的 system prompt 会多 5 段 markdown——长上下文成本会显著上升。

建议：

- 通用流程（dev、review）→ `always: true`
- 专用流程（特定项目类型）→ `always: false`，按需通过 skill 召唤

### 6. 给 `output_template` 写人类可读的 markdown

工作流的最终输出是给人看的，不是给 LLM 看的。直接返回 `{"context": {...}}` JSON 是糟糕的 UX。

```yaml
output_template: |
  ## ✅ Dev Workflow Complete
  
  ### Final Review
  {{review.reason}}
  
  ### Implementation
  {{implement}}
  
  ---
  *Generated in {{plan}} planning step → {{implement}} implementation*
```

### 7. 参数描述要详细

`parameters.properties.<name>.description` 是 LLM 决定要不要调用工具、要怎么填参的关键依据。

不好：

```yaml
task:
  type: string
  description: "task"        # 等于没写
```

好：

```yaml
task:
  type: string
  description: "Specific coding task to perform, e.g. 'Add login form validation to /auth/login route'"
```

## 常见陷阱

### 陷阱 1：忘了 `evaluate` 步骤需要 JSON 输出

```yaml
review:
  prompt: "Is the code good?"   # ← 没要求 JSON 格式
  evaluate:
    on_pass: "DONE"
    on_fail: "implement"
```

LLM 会返回散文，verdict 解析失败 → 视为 FAIL → 重试 3 次 → workflow 报错。

**修复**：prompt 显式要求 `{"verdict":"...", "reason":"..."}`。

### 陷阱 2：YAML 字段拼错被静默吃掉？现在不会了

旧版本对 YAML 容忍未知字段，所以 `condtion: ...`（少了一个 i）会被静默丢弃。

现在 `deny_unknown_fields` 已生效，加载时直接报错：

```
Failed to parse workflow YAML from ...: unknown field `condtion`, expected one of ...
```

### 陷阱 3：步骤定义了但不可达

```yaml
start_step: "a"
steps:
  a:
    prompt: "..."
    next: "DONE"
  b:                # ← 定义了但没人指向它
    prompt: "..."
    next: "DONE"
```

加载时报错：

```
Workflow has unreachable steps: ["b"]
```

**修复**：让 `b` 被某个 `next` / `on_pass` / `on_fail` 引用，或者删掉它。

### 陷阱 4：`mode: "skil"` 拼错

`mode` 是强类型枚举，只接受 `"tool"` 和 `"skill"`：

```
Failed to parse workflow YAML from ...: unknown variant `skil`, expected `tool` or `skill`
```

### 陷阱 5：循环但没有 `max_retries` 收敛点

```yaml
a:
  prompt: "..."
  next: "b"
b:
  prompt: "..."
  next: "a"        # ← 无条件回到 a，会撞 MAX_WORKFLOW_STEPS (100) 才退
```

非评审节点的 `next` 是**无条件跳转**，不会自动停。100 步上限只是兜底，撞上去就是 100 次子 agent 调用浪费掉。循环节点必须用 `evaluate` 配 `max_retries`。

### 陷阱 6：`{{step_name}}` 用了不存在的步骤

```yaml
prompt: "Refine based on {{nonexistent}}"
```

`nonexistent` 不在 context_map 中，模板原样保留 `{{nonexistent}}`，传给 LLM 看到的就是字面量。**不会报错，但 LLM 行为不可预测**。

写 prompt 时检查所有 `{{...}}` 占位符的来源：要么是 `input.<param>`，要么是某个先前步骤的 name。

## 调试与排错

### 查看 workflow 是否加载

```bash
# 启动 gasket，关注日志：
[INFO] Discovered workflow 'dev_workflow' from "..."
[INFO] Discovered workflow-skill 'dev_workflow' from "..."
```

加载失败的话日志里会有 warn：

```
[WARN] Failed to validate workflow from "...": Workflow has unreachable steps: [...]
```

### 查看每步执行

`run_step` 用 `tracing::info!` 输出：

```
[Workflow dev_workflow] Step 'research' spawning subagent
[Workflow dev_workflow] Step 'research' completed (tools_used: 3)
```

`#[tracing::instrument(name = "tool.workflow")]` span 包裹了整个 execute，可以在分布式追踪后端按 workflow 名字过滤。

### verdict 解析失败

```
[WARN] [Workflow ...] Verdict parse failed for step 'review': No JSON object found in output
```

去看子 agent 的最终输出（流式事件里的 `subagent_completed`），通常是 LLM 没按要求输出 JSON。修 prompt。

### 单元测试

`tools/workflow.rs` 的 tests 模块覆盖了 manifest 解析、verdict 解析、graph 验证等核心路径。新增 workflow 字段时，跟一组测试：

```bash
cargo test -p gasket-engine --lib workflow
```

## 与子 agent 的关系

tool 模式的每个步骤都通过 `SubagentSpawner::spawn_with_stream` 启动一个独立 agent。这意味着：

- 每步是**全新对话上下文**——上一步的输出通过 prompt 显式传入
- 每步有独立的工具集（继承自父 context）和模型选择
- 流式事件会转发到前端（`subagent_started` / `subagent_completed`）

skill 模式则没有子 agent——所有步骤都在父 agent 的对话循环里执行，依赖对话历史自然传递上下文。

## 何时不该用 workflow

- **单步任务**：直接让 LLM 做就行，workflow 是额外开销
- **流程会频繁变化**：YAML 不是 prompt engineering 的好载体，每次改完都要重启
- **步骤间需要复杂的数据变换**：模板替换是简单字符串替换，没有逻辑能力——这种场景应该写代码
- **需要并行**：当前 workflow 是严格顺序的状态机，并行需要用 `spawn_parallel` 工具

## 参考

- 源码：`gasket/engine/src/tools/workflow.rs`
- skill 模式转换：`gasket/engine/src/skills/workflow_skill.rs`
- 工具注册：`gasket/engine/src/tools/builder.rs`
- 内置示例：`workspace/workflows/dev.yaml`、`workspace/workflows/self-evolution.yaml`
