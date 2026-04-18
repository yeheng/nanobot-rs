# 记忆与历史模块

> AI 是如何记住事情的？—— 记忆与历史系统详解

---

## 一句话理解

把 Gasket 想象成一个人：
- **历史** = 短期记忆（刚才说了什么）
- **记忆** = 长期记忆（你的名字、喜好、过去的约定）

---

## 历史系统（短期记忆）

### 是什么？

历史系统记录的是**当前这次对话**的内容。就像你和朋友聊天时，你会记得刚才说了什么，基于上文继续对话。

```mermaid
flowchart LR
    A[用户: 你好] --> B[AI: 你好！有什么可以帮忙？]
    B --> C[用户: 帮我写代码]
    C --> D[AI: 写什么语言的代码？]
    D --> E[用户: Python]
    E --> F[AI: 好的，Python代码如下...]
```

### 为什么需要历史？

没有历史，AI 每次回复都是独立的，无法连贯对话：

| 有历史 | 无历史 |
|--------|--------|
| 用户: 帮我写代码 | 用户: 帮我写代码 |
| AI: 写什么语言？ | AI: 好的，写什么代码？ |
| 用户: Python | 用户: Python |
| AI: 好的，Python代码... | AI: （不知道用户要什么代码）|

### 历史数据流

```mermaid
sequenceDiagram
    participant U as 用户
    participant S as Session
    participant H as 历史存储
    participant AI as AI大脑

    U->>S: 发送消息
    S->>H: 保存用户消息
    S->>H: 读取最近对话历史
    H-->>S: 返回历史记录
    S->>AI: 历史 + 新消息
    AI-->>S: 生成回复
    S->>H: 保存AI回复
    S-->>U: 返回回复
```

### 历史太多怎么办？

对话太长时，历史会占用大量空间。这时候需要**摘要压缩**：

```mermaid
flowchart TB
    subgraph 完整历史
        A1[用户: 你好]
        A2[AI: 你好]
        A3[用户: 帮我写Python代码]
        A4[AI: 好的，什么功能？]
        A5[用户: 计算器]
        A6[AI: （100行代码）]
        A7[用户: 再加个功能]
        A8[AI: （200行代码）]
        ...
    end
    
    B{Token超限?}
    
    subgraph 压缩后
        C[摘要: 用户要写Python计算器，已实现加减乘除]
        D[保留 watermark 之后的事件]
    end
    
    A1 --> B
    B -->|是| C
    B -->|否| D
```

---

## 记忆系统（长期记忆）

### 是什么？

记忆系统存储的是**跨会话的知识**。比如：
- 用户的名字、职业
- 用户的代码风格偏好
- 之前项目的技术选型
- 用户喜欢的沟通方式

```mermaid
flowchart LR
    subgraph 今天对话
        A[用户: 我叫小明]
    end
    
    subgraph 明天对话
        B[用户: 你好] 
        C[AI: 你好小明！今天想做什么？]
    end
    
    A -.保存到记忆.-> M[(长期记忆)]
    M -.读取.-> C
```

### 记忆的六个抽屉

记忆按用途分为六个场景（像六个抽屉）：

```mermaid
graph TB
    M[记忆柜]
    M --> P[Profile抽屉<br/>用户信息]
    M --> A[Active抽屉<br/>当前工作]
    M --> K[Knowledge抽屉<br/>学到的知识]
    M --> D[Decisions抽屉<br/>做过的决定]
    M --> E[Episodes抽屉<br/>经历/事件]
    M --> R[Reference抽屉<br/>参考资料]
    
    P --> P1[用户叫小明]
    P --> P2[偏好简洁回答]
    
    A --> A1[正在做网站项目]
    A --> A2[待办: 修复bug]
    
    K --> K1[Python异步模式]
    K --> K2[Rust所有权规则]
```

### 记忆的冷热分层

不是所有记忆都同等重要，系统会自动管理：

```mermaid
flowchart TB
    subgraph Hot热记忆
        H1[用户名字]
        H2[当前项目]
        H3[今天的任务]
    end
    
    subgraph Warm温记忆
        W1[上周的技术讨论]
        W2[上月的设计方案]
    end
    
    subgraph Cold冷记忆
        C1[半年前的项目]
        C2[一年前的约定]
    end
    
    subgraph Archived归档
        A1[很久以前的项目]
    end
    
    H1 -->|7天未访问 → Warm| W1
    W1 -->|30天未访问 → Cold| C1
    C1 -->|90天未访问 → Archived| A1
```

### 三阶段记忆加载

当用户提问时，系统分三个阶段查找记忆（总预算约 4000 tokens）：

| 阶段 | 预算 | 内容 | 加载策略 |
|------|------|------|---------|
| **Bootstrap** | ~1500 tokens | Profile + Active (Hot/Warm) | 必加载 |
| **Scenario** | ~1500 tokens | 当前场景 Hot + tag 匹配 | 条件加载 |
| **On-demand** | ~1000 tokens | 语义搜索补充 | 按需加载 |

排序优先级：**豁免场景优先** → **skill 类型优先** → **高频优先** → **相似度优先**

**加载结果以 User Message 注入**（而非追加到 System Prompt），这是为了保护 Anthropic 等平台的 Prompt Cache：System Prompt 在整个对话中保持不变，只有动态的 memory 内容作为 User Message 每轮变化。长会话可节省 90%+ 的 API 成本。

```mermaid
sequenceDiagram
    participant U as 用户提问
    participant S as 系统
    participant P as Profile抽屉
    participant A as Active抽屉
    participant K as Knowledge抽屉
    participant Se as 搜索引擎
    
    U->>S: "帮我优化代码"
    
    Note over S: 阶段1：必查记忆
    S->>P: 加载用户信息
    P-->>S: 小明，偏好Python
    S->>A: 加载当前工作
    A-->>S: 正在做Web项目
    
    Note over S: 阶段2：场景记忆
    S->>K: 查找代码优化相关
    K-->>S: 找到Python优化技巧
    
    Note over S: 阶段3：按需搜索
    S->>Se: 语义搜索相关记忆
    Se-->>S: 找到之前的性能调优笔记
    
    S-->>U: 基于记忆生成回复
```

---

## 记忆的类型：Note vs Skill

记忆不仅按**场景**分类，还按**类型**区分用途：

| 类型 | 用途 | 示例 | 加载优先级 |
|------|------|------|-----------|
| **note** (默认) | 事实性知识 | 用户偏好、代码片段、项目信息 | 正常 |
| **skill** | 程序性知识 | 部署流程、调试步骤、SOP | **优先** |

**何时使用 skill 类型？**
- 内容包含明确的步骤（1. 2. 3.）
- 有陷阱提示和验证方法
- 是跨会话可复用的操作流程

调用 `memorize` 时，通过 `memory_type: "skill"` 标记。skill 类型的记忆在三阶段加载中会被优先排序。

---

## 记忆 vs 历史：完整对比

```mermaid
graph LR
    subgraph 历史系统
        H1[短期记忆]
        H2[当前对话]
        H3[自动保存]
        H4[不会丢失]
        H5[按时间顺序]
    end
    
    subgraph 记忆系统
        M1[长期记忆]
        M2[跨会话]
        M3[需要写入]
        M4[结构化存储]
        M5[按场景分类]
    end
    
    H1 --> M1
    H2 --> M2
    H3 --> M3
    H4 --> M4
    H5 --> M5
```

| 特性 | 历史 | 记忆 |
|------|------|------|
| **保存什么** | 对话内容 | 关键信息、知识、约定 |
| **保存多久** | 当前会话 | 永久 |
| **谁决定存** | 自动 | AI或用户决定 |
| **怎么存** | 按时间线 | 按场景分类 |
| **怎么用** | 直接拼接 | 智能匹配查询 |

---

## 数据流动全景图

```mermaid
flowchart TB
    subgraph 输入
        U[用户消息]
    end
    
    subgraph 处理层
        H[历史系统]
        M[记忆系统]
        C[上下文组装]
    end
    
    subgraph 存储层
        HS[(历史存储<br/>SQLite)]
        MS[(记忆存储<br/>Markdown文件)]
        ME[(嵌入索引<br/>SQLite)]
    end
    
    subgraph 输出
        AI[AI大脑]
        R[回复]
    end
    
    U --> H
    U --> M
    H <--> HS
    M <--> MS
    M <--> ME
    H --> C
    M --> C
    C --> AI
    AI --> R
    AI --> H
```

---

## 实际使用场景

### 场景1：记住用户名字

```mermaid
sequenceDiagram
    participant U as 用户
    participant AI as AI
    participant Mem as 记忆系统
    
    U->>AI: 我叫小明
    AI->>Mem: 保存: 用户名字=小明
    Note over Mem: 存入Profile抽屉

    Note over U,Mem: 第二天
    
    U->>AI: 你好
    AI->>Mem: 加载用户记忆
    Mem-->>AI: 用户叫小明
    AI-->>U: 你好小明！今天想做什么？
```

### 场景2：跨会话继续项目

```mermaid
sequenceDiagram
    participant U as 用户
    participant AI as AI
    participant A as Active抽屉
    participant K as Knowledge抽屉
    
    U->>AI: 我要做一个电商网站
    AI->>A: 保存: 当前项目=电商网站
    AI->>K: 保存: 技术栈=React+Node.js
    
    Note over U,K: 一周后
    
    U->>AI: 继续那个项目
    AI->>A: 查询Active抽屉
    A-->>AI: 电商网站项目
    AI->>K: 查询相关知识
    K-->>AI: 技术栈信息
    AI-->>U: 好的，我们继续电商网站，上次选的是React+Node.js...
```

### 场景3：智能召回相关知识

```mermaid
sequenceDiagram
    participant U as 用户
    participant AI as AI
    participant M as 记忆系统
    
    U->>AI: 怎么优化这段代码？
    
    AI->>M: 搜索"代码优化"
    M->>M: 语义匹配
    Note over M: 找到之前关于<br/>Python性能优化的笔记
    
    M-->>AI: 返回相关记忆
    AI-->>U: 根据你之前的笔记，可以试试这些方法...
```

---

## 常见问题

**Q: 历史会保存多久？**
A: 历史是永久保存的，但旧的对话会被压缩成摘要，只保留关键信息。

**Q: 记忆会自己学习吗？**
A: AI 会根据对话自动判断哪些信息值得记住，也可以通过 `memorize` 工具主动保存。

**Q: 记忆会不会太多？**
A: 系统会自动管理：不常用的记忆会逐渐"降温"，很久不用的会归档，不会无限增长。

**Q: 隐私安全吗？**
A: 所有数据都存在你自己的电脑上，不会上传到云端。
