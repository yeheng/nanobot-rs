# 子代理系统 (Subagents)

> AI 也能当老板——让 AI 创建分身并行工作

---

## 一句话理解

子代理就是 AI 能创建**分身**来并行处理多个任务，就像老板把工作分配给不同员工。

```mermaid
flowchart TB
    Boss[主AI<br/>老板]
    Task[复杂任务]
    
    Boss --> Task
    Task --> A[子代理1<br/>员工A]
    Task --> B[子代理2<br/>员工B]
    Task --> C[子代理3<br/>员工C]
    
    A --> Result[汇总结果]
    B --> Result
    C --> Result
    Result --> Boss
```

---

## 为什么需要子代理

### 单 AI 处理的局限

```mermaid
flowchart TB
    subgraph 一个人干所有活
        A[用户: 分析这个项目的100个文件]
        B[AI: 逐个分析]
        C[文件1]
        D[文件2]
        E[...]
        F[文件100]
        G[总耗时: 100分钟]
    end
    
    A --> B
    B --> C
    C --> D
    D --> E
    E --> F
    F --> G
    
    style G fill:#FFCDD2
```

### 多子代理并行处理

```mermaid
flowchart TB
    subgraph 多个人并行干
        A[用户: 分析100个文件]
        B[主AI: 分配给10个子代理]
        
        subgraph 并行处理
            C1[子代理1<br/>文件1-10]
            C2[子代理2<br/>文件11-20]
            C3[...]
            C4[子代理10<br/>文件91-100]
        end
        
        D[汇总分析结果]
        E[总耗时: 10分钟]
    end
    
    A --> B
    B --> C1
    B --> C2
    B --> C3
    B --> C4
    C1 --> D
    C2 --> D
    C3 --> D
    C4 --> D
    D --> E
    
    style E fill:#C8E6C9
```

**优势：**
- ⚡ **更快**：并行处理，10 倍速提升
- 🎯 **更专注**：每个子代理专注一个子任务
- 🔄 **更灵活**：可以递归创建子-子代理
- 💪 **更强**：处理单 AI 搞不定的复杂任务

---

## 子代理 vs 主代理

```mermaid
graph TB
    subgraph 对比
        Main[主代理<br/>Persistent Session]
        Sub[子代理<br/>Stateless Session]
    end
    
    subgraph 主代理特点
        M1[保存对话历史 ✓]
        M2[记住用户信息 ✓]
        M3[长期运行]
        M4[资源占用高]
    end
    
    subgraph 子代理特点
        S1[不保存历史 ✗]
        S2[一次性使用]
        S3[轻量级]
        S4[用完即走]
    end
    
    Main --> M1
    Main --> M2
    Main --> M3
    Main --> M4
    
    Sub --> S1
    Sub --> S2
    Sub --> S3
    Sub --> S4
```

| 特性 | 主代理 | 子代理 |
|------|-------|-------|
| **对话历史** | 保存 | 不保存 |
| **用户记忆** | 有 | 无（继承主代理配置）|
| **使用场景** | 日常对话 | 临时任务 |
| **生命周期** | 长期 | 一次性的 |
| **比喻** | 全职员工 | 外包临时工 |

---

## 创建子代理的方式

### 方式1：单个子代理（spawn）

```mermaid
sequenceDiagram
    participant User as 用户
    participant Main as 主AI
    participant Sub as 子代理
    
    User->>Main: 分析这个复杂的代码文件
    
    Note over Main: 文件太复杂，需要专门分析
    
    Main->>Sub: 创建子代理
    Note over Sub: 轻量级Session<br/>专注分析代码
    
    Sub->>Sub: 分析代码...
    Sub-->>Main: 返回分析结果
    
    Note over Sub: 子代理销毁
    
    Main-->>User: 根据分析结果回复
```

**参数：**
- `task`（必填）- 任务描述
- `model_id`（可选）- 模型 Profile ID，如 `"fast"`、`"coder"`，使用 `agents.models` 中定义的模型

**适用场景：**
- 复杂代码分析
- 长篇文档总结
- 独立的研究任务

### 方式2：并行多个子代理（spawn_parallel）

```mermaid
sequenceDiagram
    participant User as 用户
    participant Main as 主AI
    participant Sub1 as 子代理1
    participant Sub2 as 子代理2
    participant Sub3 as 子代理3
    
    User->>Main: 对比这三个方案
    
    Main->>Sub1: 分析方案A
    Main->>Sub2: 分析方案B
    Main->>Sub3: 分析方案C
    
    par 并行执行
        Sub1->>Sub1: 分析方案A（实时事件→用户）
    and
        Sub2->>Sub2: 分析方案B（实时事件→用户）
    and
        Sub3->>Sub3: 分析方案C（实时事件→用户）
    end
    
    Main->>Main: 综合对比
    Main-->>User: 三个方案对比结果
```

**参数：**
- `tasks`（必填）- 任务列表，支持两种格式：
  - 简单字符串数组：`["任务A", "任务B"]`
  - 带模型选择的对象数组：`[{"task": "任务A", "model_id": "fast"}, ...]`
- 最多 10 个任务，最多 5 个并发执行（防止 API 限流）

**适用场景：**
- A/B/C 方案对比
- 批量文件处理
- 并行数据收集

---

## 子代理的工作流程

```mermaid
flowchart TB
    subgraph 创建阶段
        A[主AI决定创建子代理] --> B[指定任务描述]
        B --> C[可选:指定模型]
        C --> D[创建Stateless Session]
    end
    
    subgraph 执行阶段
        D --> E[加载系统提示]
        E --> F[发送任务]
        F --> G[子AI独立工作]
        
        G --> H{需要工具?}
        H -->|是| I[调用工具]
        I --> G
        H -->|否| J[生成结果]
    end
    
    subgraph 收尾阶段
        J --> K[返回结果给主AI]
        K --> L[子代理销毁]
    end
    
    style D fill:#E3F2FD
    style L fill:#FFCDD2
```

### 详细时序

```mermaid
sequenceDiagram
    participant Main as 主AI
    participant Spawner as 子代理创建器
    participant Sub as 子代理Session
    participant Kernel as Kernel大脑
    participant Tools as 工具
    
    Main->>Spawner: spawn(task="分析代码")
    
    Spawner->>Sub: 创建Stateless Session
    activate Sub
    
    Note over Sub: 独立运行
    
    Sub->>Kernel: 执行任务
    
    loop 思考过程
        Kernel-->>Sub: 需要读文件
        Sub->>Tools: read_file
        Tools-->>Sub: 文件内容
        Sub->>Kernel: 继续思考
    end
    
    Kernel-->>Sub: 分析完成
    Sub-->>Spawner: 返回结果
    deactivate Sub
    
    Spawner-->>Main: 子代理结果
    
    Note over Sub: 子代理已销毁
```

---

## 子代理追踪器

管理多个子代理，等待所有结果：

```mermaid
flowchart TB
    subgraph SubagentTracker
        T[追踪器]
        R[结果接收器]
        E[事件接收器]
    end
    
    subgraph 子代理们
        S1[子代理1]
        S2[子代理2]
        S3[子代理3]
    end
    
    subgraph 结果
        Res[汇总结果]
    end
    
    T --> R
    T --> E
    
    S1 -.->|结果| R
    S2 -.->|结果| R
    S3 -.->|结果| R
    
    S1 -.->|事件| E
    S2 -.->|事件| E
    S3 -.->|事件| E
    
    R --> Res
```

```mermaid
sequenceDiagram
    participant Main as 主AI
    participant Tracker as 追踪器
    participant Sub1 as 子代理1
    participant Sub2 as 子代理2
    participant Sub3 as 子代理3
    
    Main->>Tracker: 创建追踪器
    Tracker-->>Main: 返回发送器
    
    Main->>Sub1: 创建(使用Tracker发送器)
    Main->>Sub2: 创建(使用Tracker发送器)
    Main->>Sub3: 创建(使用Tracker发送器)
    
    par 并行执行
        Sub1->>Tracker: 结果1
    and
        Sub2->>Tracker: 结果2
    and
        Sub3->>Tracker: 结果3
    end
    
    Main->>Tracker: wait_for_all(3)
    Tracker-->>Main: [结果1, 结果2, 结果3]
```

---

## 流式事件转发

子代理的执行过程可以实时看到：

```mermaid
sequenceDiagram
    participant Sub as 子代理
    participant Inject as 注入agent_id
    participant Main as 主AI
    participant User as 用户
    
    Sub->>Inject: 思考: "分析中..."
    Inject->>Inject: 添加agent_id="sub-001"
    Inject->>Main: [sub-001] 思考: "分析中..."
    Main->>User: 显示: [子代理1] 分析中...
    
    Sub->>Inject: 调用工具: read_file
    Inject->>Inject: 添加agent_id="sub-001"
    Inject->>Main: [sub-001] 调用工具
    Main->>User: 显示: [子代理1] 读取文件...
    
    Sub->>Inject: 完成
    Inject->>Main: [sub-001] 完成
    Main->>User: 显示: [子代理1] 完成
```

**WebSocket 模式下可接收的事件类型：**

| ChatEvent | 说明 |
|-----------|------|
| `subagent_started` | 子代理启动，附带任务描述 |
| `subagent_thinking` | 子代理的思考过程 |
| `subagent_tool_start` | 子代理开始调用工具 |
| `subagent_tool_end` | 子代理工具调用完成 |
| `subagent_content` | 子代理生成的内容片段 |
| `subagent_completed` | 子代理完成，返回结果摘要 |
| `subagent_error` | 子代理执行出错 |

**这样用户能看到：**
- `[子代理1]` 正在分析代码...
- `[子代理1]` 正在读取文件 main.py...
- `[子代理1]` 分析完成

---

## 实际使用场景

### 场景1：代码审查

```mermaid
flowchart TB
    User[用户: 审查这个PR] --> Main[主AI]
    
    Main --> Split[分解任务]
    
    Split --> A[子代理1<br/>审查安全性]
    Split --> B[子代理2<br/>审查性能]
    Split --> C[子代理3<br/>审查代码风格]
    Split --> D[子代理4<br/>审查逻辑]
    
    A --> Merge[汇总审查意见]
    B --> Merge
    C --> Merge
    D --> Merge
    
    Merge --> Report[生成审查报告]
    Report --> User
```

### 场景2：多数据源收集

```mermaid
sequenceDiagram
    participant User as 用户
    participant Main as 主AI
    participant News as 子代理-新闻
    participant Social as 子代理-社交
    participant Blog as 子代理-博客
    
    User->>Main: 收集关于AI的最新动态
    
    Main->>News: 搜索新闻网站
    Main->>Social: 搜索社交媒体
    Main->>Blog: 搜索技术博客
    
    par 并行收集
        News->>News: 搜索新闻
        News-->>Main: 新闻结果
    and
        Social->>Social: 搜索社媒
        Social-->>Main: 社媒结果
    and
        Blog->>Blog: 搜索博客
        Blog-->>Main: 博客结果
    end
    
    Main->>Main: 汇总整理
    Main-->>User: AI最新动态总结
```

### 场景3：递归分解任务

```mermaid
flowchart TB
    U[写一本关于Python的书]
    
    U --> M1[主代理: 规划章节]
    
    M1 --> S1[子代理: 写第1章基础]
    M1 --> S2[子代理: 写第2章进阶]
    M1 --> S3[子代理: 写第3章实战]
    
    S2 --> S2_1[子-子代理: 2.1节装饰器]
    S2 --> S2_2[子-子代理: 2.2节生成器]
    S2 --> S2_3[子-子代理: 2.3节异步]
    
    S1 --> Compile[汇总编辑]
    S2_1 --> Compile
    S2_2 --> Compile
    S2_3 --> Compile
    S3 --> Compile
    
    Compile --> Book[完整书籍]
```

---

## 模型选择

子代理可以使用不同的 AI 模型：

```mermaid
graph TB
    subgraph 主代理
        Main[使用GPT-4<br/>处理复杂对话]
    end
    
    subgraph 子代理们
        Sub1[使用GPT-3.5<br/>简单分析任务]
        Sub2[使用Claude<br/>长文本总结]
        Sub3[使用本地模型<br/>敏感数据处理]
    end
    
    Main --> Sub1
    Main --> Sub2
    Main --> Sub3
```

**策略：**
- 主任务用强模型（GPT-4/Claude-3）
- 简单子任务用快模型（GPT-3.5）
- 特定任务用专门模型（代码/CodeLlama）

---

## 超时和错误处理

```mermaid
flowchart TB
    subgraph 子代理执行
        Start[创建子代理] --> Run[开始执行]
        Run --> Timeout{超时?}
        
        Timeout -->|否| Complete[正常完成]
        Timeout -->|是|10分钟| Fail[超时失败]
        
        Run --> Error{出错?}
        Error -->|是| Fail
        Error -->|否| Complete
    end
    
    subgraph 主代理处理
        Complete --> Result[返回结果]
        Fail --> Retry[重试/跳过]
        Retry --> Result
    end
    
    style Timeout fill:#FFD700
    style Fail fill:#FFCDD2
    style Complete fill:#C8E6C9
```

**超时配置：**
- 子代理执行超时：`agents.defaults.subagent_timeout_secs`（默认 600 秒 = 10 分钟）
- 工具执行超时：`agents.defaults.tool_timeout_secs`（默认 120 秒）
- 失败返回错误信息，主代理决定是否重试

---

## 与工具系统集成

子代理本身就是通过 `spawn` 工具调用的：

```mermaid
sequenceDiagram
    participant User as 用户
    participant Main as 主AI
    participant Spawn as spawn工具
    participant Sub as 子代理系统
    
    User->>Main: 并行分析这5个文件
    
    Main->>Main: 决定使用spawn_parallel
    
    Main->>Spawn: 调用spawn_parallel
    
    loop 5个文件
        Spawn->>Sub: 创建子代理
        Sub->>Sub: 分析文件
        Sub-->>Spawn: 返回结果
    end
    
    Spawn-->>Main: 汇总5个结果
    Main-->>User: 完整分析报告
```

---

## 常见问题

**Q: 子代理和主代理共享记忆吗？**
A: 子代理是无状态的，不保存历史。但可以继承主代理的配置和上下文。

**Q: 可以创建多少个子代理？**
A: `spawn_parallel` 一次最多 10 个任务，内部最多 5 个并发执行。超过会报错。

**Q: 子代理可以创建子-子代理吗？**
A: 可以！支持递归创建，适合层层分解的复杂任务。

**Q: WebSocket 模式下能看到子代理的执行过程吗？**
A: 可以。子代理的思考、工具调用和内容生成会实时推送到前端，用户可以看到每个子代理的进度。

**Q: 子代理摘要太长怎么办？**
A: 可通过 `agents.defaults.ws_summary_limit` 限制摘要长度（字符数），0 表示不限制。
