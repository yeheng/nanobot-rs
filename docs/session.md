# 会话管理模块 (Session)

> 对话的大管家——如何组织一次完整的 AI 对话

---

## 一句话理解

Session 是 AI 对话的**大管家**，负责接待用户、准备资料、协调大脑工作、整理回复。

```mermaid
flowchart TB
    U[用户] --> S[Session管家]
    S --> P[准备资料]
    S --> K[Kernel大脑]
    S --> R[整理回复]
    K --> R
    R --> U
```

---

## 生活中的类比

Session 就像一个**私人助理**：

```mermaid
flowchart TB
    subgraph 见客户前
        A1[了解客户背景]
        A2[准备相关资料]
        A3[整理历史记录]
    end
    
    subgraph 见客户时
        B1[记录谈话内容]
        B2[适时递上资料]
        B3[协调专家回答]
    end
    
    subgraph 见客户后
        C1[整理会议纪要]
        C2[更新客户档案]
        C3[安排后续工作]
    end
    
    A1 --> A2 --> A3 --> B1 --> B2 --> B3 --> C1 --> C2 --> C3
```

| 助理工作 | Session 对应功能 |
|---------|-----------------|
| 了解客户背景 | 加载用户记忆 |
| 准备资料 | 组装系统提示、技能 |
| 整理历史记录 | 加载对话历史 |
| 记录谈话 | 保存消息到历史 |
| 协调专家 | 调用 Kernel |
| 整理纪要 | 上下文压缩 |
| 更新档案 | 更新记忆 |

---

## Session 的核心职责

```mermaid
mindmap
  root((Session管家))
    对话前准备
      加载系统提示
      加载用户记忆
      加载对话历史
      准备相关技能
    对话中协调
      接收用户输入
      执行前置钩子
      调用AI大脑
      处理流式输出
    对话后整理
      保存回复到历史
      触发后置钩子
      执行上下文压缩
      更新访问记录
```

---

## 完整对话流程

### 阶段一：准备（对话前）

```mermaid
sequenceDiagram
    participant U as 用户
    participant S as Session管家
    participant H as 历史系统
    participant M as 记忆系统
    participant SK as 技能系统
    participant C as 上下文组装
    
    Note over S: 用户发起对话
    
    S->>SK: 加载系统提示
    SK-->>S: 你是谁/能力/规则
    
    S->>M: 查询用户记忆
    M-->>S: 用户背景信息
    
    S->>SK: 加载相关技能
    SK-->>S: 技能上下文
    
    S->>H: 读取对话历史
    H-->>S: 最近消息
    
    S->>C: 组装完整上下文
    C-->>S: 准备好的提示词
```

### 阶段二：执行（对话中）

```mermaid
sequenceDiagram
    participant U as 用户
    participant S as Session管家
    participant Hook as 钩子系统
    participant K as Kernel大脑
    participant Out as 输出处理
    
    U->>S: 发送消息
    
    S->>S: 保存用户消息
    
    S->>Hook: 执行前置钩子
    Hook-->>S: 继续/中止
    
    alt 钩子中止
        S-->>U: 返回中止消息
    else 继续处理
        S->>K: 发送完整上下文
        
        Note over K: AI思考中...
        
        loop 流式输出
            K-->>S: 输出片段
            S-->>U: 实时显示
        end
        
        K-->>S: 完整回复
    end
```

### 阶段三：收尾（对话后）

```mermaid
sequenceDiagram
    participant S as Session管家
    participant H as 历史系统
    participant Hook as 钩子系统
    participant Comp as 压缩器
    participant M as 记忆系统
    
    S->>H: 保存AI回复
    
    S->>Hook: 执行后置钩子
    Hook-->>S: 完成
    
    alt 历史太长
        S->>Comp: 触发压缩
        Comp->>Comp: 生成摘要
        Comp->>H: 保存摘要
    end
    
    S->>M: 更新访问记录
    
    Note over S: 等待下一次对话
```

---

## 上下文组装过程

Session 把各种信息组装成 AI 能理解的格式：

```mermaid
flowchart TB
    subgraph 原材料
        S[系统提示<br/>你是谁/规则]
        SK[技能<br/>专项能力]
        SUM[摘要<br/>之前聊了什么]
        H[历史<br/>最近对话]
        M[记忆<br/>用户信息]
        Q[当前问题]
    end
    
    subgraph 组装过程
        A[上下文组装器]
    end
    
    subgraph 成品
        F[完整提示词]
    end
    
    S --> A
    SK --> A
    SUM --> A
    H --> A
    M --> A
    Q --> A
    
    A --> F
    
    style F fill:#C8E6C9
```

### 组装示例

```
┌─────────────────────────────────────────┐
│ 系统提示                                 │
│ 你是Gasket，一个AI助手...                │
├─────────────────────────────────────────┤
│ 技能                                    │
│ 你擅长Python开发...                      │
├─────────────────────────────────────────┤
│ 摘要                                    │
│ 之前聊了：用户在做一个网站项目...         │
├─────────────────────────────────────────┤
│ 历史                                    │
│ 用户: 帮我写个登录功能                   │
│ AI: 好的，用什么框架？                   │
│ 用户: React                              │
├─────────────────────────────────────────┤
│ 记忆                                    │
│ 用户叫小明，后端工程师                    │
├─────────────────────────────────────────┤
│ 当前问题                                 │
│ 用户: 继续                               │
└─────────────────────────────────────────┘
```

---

## 两种类型的 Session

```mermaid
graph TB
    subgraph Session类型
        P[持久化Session<br/>Persistent]
        S[无状态Session<br/>Stateless]
    end
    
    subgraph 持久化特点
        P1[保存对话历史]
        P2[记住用户信息]
        P3[跨会话记忆]
        P4[主AI使用]
    end
    
    subgraph 无状态特点
        S1[不保存历史]
        S2[一次性使用]
        S3[轻量级]
        S4[子代理使用]
    end
    
    P --> P1
    P --> P2
    P --> P3
    P --> P4
    
    S --> S1
    S --> S2
    S --> S3
    S --> S4
```

| 特性 | 持久化 Session | 无状态 Session |
|------|---------------|----------------|
| **保存历史** | ✓ | ✗ |
| **保存记忆** | ✓ | ✗ |
| **使用场景** | 主对话 | 子任务 |
| **资源占用** | 较高 | 很低 |
| **举例** | 你和AI的日常对话 | 让AI分析一个文件的临时任务 |

---

## 上下文压缩

当对话太长时，Session 会进行"总结"：

```mermaid
flowchart TB
    subgraph 压缩前
        A1[用户: 你好]
        A2[AI: 你好]
        A3[用户: 讲个故事]
        A4[AI: 从前有座山...]
        A5[...50轮对话...]
        A6[用户: 后来呢]
        A7[AI: 后来...]
    end
    
    C{Token超限?}
    
    subgraph 压缩后
        S[摘要: 用户让讲童话故事<br/>故事内容：王子救公主]
        R[保留最近3轮]
    end
    
    A1 --> C
    C -->|是| S
    C -->|否| A7
    A7 --> R
    
    style S fill:#FFD700
```

**压缩策略：**
- 老旧消息 → 生成摘要
- 最近消息 → 保留完整
- 关键信息 → 提取保存

---

## Session 与 Kernel 的关系

```mermaid
graph TB
    subgraph Session层[Session层 - 大管家]
        S1[接收用户输入]
        S2[准备上下文]
        S3[管理历史记忆]
        S4[执行钩子]
        S5[保存结果]
    end
    
    subgraph Kernel层[Kernel层 - 大脑]
        K1[纯思考]
        K2[工具调用]
        K3[生成回复]
    end
    
    subgraph 外部
        User[用户]
        AI[AI模型API]
    end
    
    User --> S1
    S1 --> S2
    S2 --> K1
    K1 --> K2
    K2 --> AI
    AI --> K2
    K2 --> K3
    K3 --> S5
    S5 --> User
    S2 --> S3
    S3 --> S2
    S4 --> K1
    K3 --> S4
    
    style Session层 fill:#E3F2FD
    style Kernel层 fill:#FFF3E0
```

**比喻：**
- **Session** = 餐厅经理（接待、安排、记录）
- **Kernel** = 厨师（专心做菜）
- **AI模型** = 食材/厨具

---

## 数据流向全景图

```mermaid
flowchart TB
    subgraph 输入层
        U[用户消息]
    end
    
    subgraph Session管理层
        R[接收]
        P[准备]
        C[协调]
        S[保存]
    end
    
    subgraph 数据存储
        HS[历史存储]
        MS[记忆存储]
        SS[摘要存储]
    end
    
    subgraph 处理层
        H[前置钩子]
        K[Kernel大脑]
        A[后置钩子]
    end
    
    subgraph 输出层
        O[AI回复]
    end
    
    U --> R
    R --> P
    
    P --> HS
    P --> MS
    HS --> P
    MS --> P
    
    P --> H
    H --> C
    C --> K
    K --> A
    A --> S
    
    S --> HS
    S --> MS
    S --> SS
    
    A --> O
    O --> U
```

---

## 实际使用场景

### 场景1：日常对话

```mermaid
sequenceDiagram
    participant U as 用户:小明
    participant S as Session
    participant M as 记忆
    participant K as Kernel
    
    Note over S: 第一次对话
    
    U->>S: 你好，我叫小明
    S->>S: 创建持久化Session
    S->>M: 保存：用户叫小明
    S->>K: 处理消息
    K-->>S: 回复
    S-->>U: 你好小明！
    
    ...第二天...
    
    Note over S: 第二次对话
    
    U->>S: 你好
    S->>S: 恢复Session
    S->>M: 查询用户记忆
    M-->>S: 用户叫小明
    S->>K: 上下文+记忆
    K-->>S: 回复
    S-->>U: 你好小明！今天想做什么？
```

### 场景2：复杂任务（使用子代理）

```mermaid
sequenceDiagram
    participant U as 用户
    participant Main as 主Session
    participant K1 as Kernel
    participant Sub as 子Session<br/> Stateless
    participant K2 as Kernel
    
    U->>Main: 分析这个项目
    
    Main->>K1: 发现需要分析多个文件
    
    par 创建子代理并行处理
        Main->>Sub: 创建子Session1
        Sub->>K2: 分析文件A
        K2-->>Sub: 结果1
        Sub-->>Main: 返回结果1
    and
        Main->>Sub: 创建子Session2
        Sub->>K2: 分析文件B
        K2-->>Sub: 结果2
        Sub-->>Main: 返回结果2
    and
        Main->>Sub: 创建子Session3
        Sub->>K2: 分析文件C
        K2-->>Sub: 结果3
        Sub-->>Main: 返回结果3
    end
    
    Main->>Main: 汇总分析结果
    Main-->>U: 完整项目分析
```

### 场景3：长对话压缩

```mermaid
sequenceDiagram
    participant U as 用户
    participant S as Session
    participant H as 历史系统
    participant C as 压缩器
    
    loop 正常对话50轮
        U->>S: 发送消息
        S->>H: 保存
        S-->>U: 回复
    end
    
    Note over S: Token接近上限
    
    U->>S: 新消息
    S->>S: 检测到需要压缩
    S->>C: 触发压缩
    
    C->>H: 读取老旧消息
    C->>C: 生成摘要
    C->>H: 保存摘要
    C-->>S: 压缩完成
    
    S->>S: 继续处理新消息
    S-->>U: 回复
```

---

## 常见问题

**Q: Session 和对话是什么关系？**
A: 一个 Session 管理一次完整的对话过程。从开始聊天到结束，Session 负责整个过程。

**Q: 重启电脑后对话还在吗？**
A: 在！持久化 Session 会把历史保存到数据库，重启后可以恢复。

**Q: 为什么需要无状态 Session？**
A: 临时任务不需要保存历史，无状态更轻量、更快。比如让 AI 临时分析一个文件。

**Q: 上下文压缩会丢失信息吗？**
A: 压缩会保留关键信息，生成摘要。最近的消息会保留完整，不会丢失重要内容。

**Q: Session 能同时处理多个用户吗？**
A: 每个用户有独立的 Session，互不干扰。Session 通过 session_key 区分不同用户。
