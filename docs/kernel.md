# AI 大脑核心 (Kernel)

> AI 是如何思考和回复的？—— 决策中枢详解

---

## 一句话理解

Kernel 是 AI 的**大脑**，负责接收信息、思考问题、调用工具、给出回答。

```mermaid
flowchart LR
    A[用户提问] --> B[Kernel大脑] --> C[思考+工具] --> D[生成回答]
```

---

## 用餐厅服务员类比

想象 Kernel 是一个超级服务员：

```mermaid
flowchart TB
    subgraph 餐厅场景
        C[顾客: 我想吃饭]
        W[服务员: Kernel]
        K[厨房: AI模型]
        T[工具: 菜单/电话/计算器]
    end
    
    C -->|1. 接收需求| W
    W -->|2. 思考| W
    W -->|3a. 简单问题<br/>直接回答| C
    W -->|3b. 需要工具<br/>查询/计算| T
    T -->|4. 工具结果| W
    W -->|5. 综合回答| C
```

| 餐厅 | AI系统 |
|------|--------|
| 顾客 | 用户 |
| 服务员 (接收需求、协调) | **Kernel** |
| 厨房 (真正做饭) | AI模型 (GPT/Claude等) |
| 菜单/电话/计算器 | 工具 (搜索/文件/执行命令) |

---

## Kernel 的核心工作流

```mermaid
sequenceDiagram
    participant U as 用户
    participant K as Kernel大脑
    participant AI as AI模型
    participant T as 工具箱
    
    U->>K: 发送问题
    
    loop 最多思考20轮
        K->>AI: 整理上下文，请求回复
        AI-->>K: 返回：思考内容 + 可能的工具调用
        
        alt 需要调用工具
            K->>K: 解析工具请求
            K->>T: 执行工具（搜索/读文件/执行命令）
            T-->>K: 返回工具结果
            K->>K: 把结果加入上下文
        else 直接回答
            K-->>U: 返回最终答案
            Note over K,U: 结束对话
        end
    end
```

---

## 思考循环详解

Kernel 使用**迭代思考**的方式工作，就像一个侦探不断收集线索：

```mermaid
flowchart TB
    Start([开始]) --> Input[接收用户输入]
    Input --> Build[组装提示词<br/>系统提示+历史+记忆]
    Build --> Ask[问AI模型]
    Ask --> Analyze{分析回复}
    
    Analyze -->|需要工具| Tool[执行工具]
    Tool --> Result[工具结果]
    Result --> Append[追加到上下文]
    Append --> Ask
    
    Analyze -->|直接回答| Output[输出答案]
    Analyze -->|达到最大轮数| Output
    Output --> End([结束])
    
    style Start fill:#90EE90
    style End fill:#FFB6C1
    style Tool fill:#FFD700
```

### 举例说明

**用户问："今天北京的天气怎么样？"**

```mermaid
sequenceDiagram
    participant U as 用户
    participant K as Kernel
    participant AI as AI模型
    participant Tool as 天气查询工具
    
    U->>K: 今天北京天气？
    K->>AI: 用户问北京天气
    AI-->>K: 我需要查询天气
    
    Note over K: 第一轮：需要工具
    
    K->>Tool: 查询北京天气
    Tool-->>K: 晴，25°C
    
    K->>AI: 用户问天气，查询结果：晴，25°C
    AI-->>K: 北京今天晴天，25度...
    
    Note over K: 第二轮：直接回答
    
    K-->>U: 北京今天天气晴朗，25°C...
```

---

## 核心组件

```mermaid
graph TB
    subgraph Kernel大脑
        E[执行器<br/>AgentExecutor]
        T[工具执行器<br/>ToolExecutor]
        R[请求处理器<br/>RequestHandler]
        C[运行时上下文<br/>RuntimeContext]
    end
    
    subgraph 输入
        I[用户消息<br/>+ 历史 + 记忆]
    end
    
    subgraph 输出
        O[AI回复]
        F[流式文字]
    end
    
    subgraph 外部
        AI[AI模型<br/>GPT/Claude]
        Tools[各种工具]
    end
    
    I --> E
    E --> R
    R --> AI
    AI --> F
    AI --> O
    E --> T
    T --> Tools
    Tools --> T
    T --> E
    C --> E
```

### 1. 执行器 (AgentExecutor)

**职责**：协调整个思考过程

```mermaid
flowchart LR
    A[接收任务] --> B[循环思考] --> C{完成?}
    C -->|否| D[调用工具] --> B
    C -->|是| E[返回结果]
```

### 2. 工具执行器 (ToolExecutor)

**职责**：并行执行多个工具

```mermaid
flowchart TB
    Input[工具调用请求] --> Split[拆分成多个任务]
    
    Split --> T1[工具1<br/>搜索网络]
    Split --> T2[工具2<br/>读取文件]
    Split --> T3[工具3<br/>执行命令]
    
    T1 --> Join
    T2 --> Join
    T3 --> Join
    
    Join[等待全部完成] --> Output[汇总结果]
    
    style Split fill:#90EE90
    style Join fill:#FFB6C1
```

### 3. 运行时上下文 (RuntimeContext)

**职责**：依赖注入容器，承载所有外部依赖

| 依赖 | 用途 |
|------|------|
| `llm_provider` | 使用哪个 AI 模型 |
| `tool_registry` | 有哪些可用工具 |
| `config` | 温度、max_tokens 等参数 |

---

## 数据流动全景图

```mermaid
flowchart TB
    subgraph 准备阶段
        U[用户输入]
        S[系统提示<br/>你是谁/能力/规则]
        H[对话历史]
        M[相关记忆]
    end
    
    subgraph Kernel处理
        A[组装完整提示]
        B[发送到AI模型]
        C[接收回复]
        D{需要工具?}
        E[执行工具]
        F[工具结果加入上下文]
    end
    
    subgraph 输出
        R[最终回复]
        L[保存到历史]
    end
    
    U --> A
    S --> A
    H --> A
    M --> A
    A --> B
    B --> C
    C --> D
    D -->|是| E
    E --> F
    F --> B
    D -->|否| R
    R --> L
```

---

## 关键特性

### 1. 纯函数设计

```mermaid
flowchart LR
    subgraph 输入
        A[上下文 用户消息 可用工具]
    end
    
    subgraph Kernel
        B[黑盒处理]
    end
    
    subgraph 输出
        C[AI回复 工具调用]
    end

    A --> B --> C

```

**好处**：
- 容易测试
- 可预测
- 方便重试

**自动重退避策略**：`backoff = (1 << retries).min(15)` 秒
- 第1次重试：2秒
- 第2次重试：4秒
- 第3次重试：8秒
- 上限：15秒
- 最大重试次数：3次（`DEFAULT_MAX_RETRIES = 3`）

### 2. 自动重试

```mermaid
sequenceDiagram
    participant K as Kernel
    participant AI as AI模型
    
    K->>AI: 发送请求
    AI--xK: 网络超时
    
    Note over K: 等待2秒（指数退避）
    
    K->>AI: 第2次尝试
    AI--xK: 服务繁忙
    
    Note over K: 等待4秒
    
    K->>AI: 第3次尝试
    AI-->>K: 成功返回
```

### 3. 最大轮数保护

防止 AI "陷入沉思" 无限循环：

```mermaid
flowchart TB
    Start([开始]) --> Count{第几轮?}
    Count -->|第1-19轮| Think[继续思考]
    Count -->|第20轮| Stop[强制结束]
    Think --> NeedMore{还需要工具?}
    NeedMore -->|是| Count
    NeedMore -->|否| Answer[返回答案]
    Stop --> Timeout[返回: 思考太久]
    Answer --> End([结束])
    Timeout --> End
```

---

## 实际使用场景

### 场景1：简单问答

```mermaid
sequenceDiagram
    participant U as 用户
    participant K as Kernel
    participant AI as AI模型
    
    U->>K: 1+1等于几？
    K->>AI: 上下文+问题
    AI-->>K: 等于2
    K-->>U: 等于2
    
    Note over U,K: 1轮完成，无需工具
```

### 场景2：需要查资料

```mermaid
sequenceDiagram
    participant U as 用户
    participant K as Kernel
    participant AI as AI模型
    participant Web as 网络搜索
    
    U->>K: 今天有什么新闻？
    
    K->>AI: 用户问今天新闻
    AI-->>K: 我需要搜索网络
    
    K->>Web: 搜索"今天新闻"
    Web-->>K: 返回10条新闻
    
    K->>AI: 搜索到这些新闻...
    AI-->>K: 总结要点...
    
    K-->>U: 今天的主要新闻有...
```

### 场景3：多工具协作

```mermaid
sequenceDiagram
    participant U as 用户
    participant K as Kernel
    participant AI as AI模型
    participant F as 读文件工具
    participant C as 执行代码工具
    
    U->>K: 分析这个CSV文件
    
    K->>AI: 用户要分析CSV
    AI-->>K: 先读取文件
    
    K->>F: 读取data.csv
    F-->>K: 文件内容
    
    K->>AI: 文件内容是...
    AI-->>K: 需要统计计算
    
    K->>C: 执行Python统计
    C-->>K: 统计结果
    
    K->>AI: 统计结果是...
    AI-->>K: 分析结论...
    
    K-->>U: 这个CSV显示...
```

---

## Kernel vs Session 的关系

```mermaid
graph TB
    subgraph Session层
        S1[管理对话状态]
        S2[加载历史记忆]
        S3[执行前后钩子]
        S4[保存回复到历史]
    end
    
    subgraph Kernel层
        K1[纯思考核心]
        K2[工具执行]
        K3[与AI模型通信]
    end
    
    subgraph 外部
        AI[AI模型API]
        Tools[各种工具]
    end
    
    S1 --> K1
    S2 --> K1
    K1 --> K2
    K1 --> K3
    K3 --> AI
    K2 --> Tools
    K1 --> S4
    
    style Session层 fill:#E1F5FE
    style Kernel层 fill:#FFF3E0
```

**比喻**：
- **Session** = 餐厅经理（接待客人、安排座位、记录订单、处理投诉）
- **Kernel** = 厨师（专心做菜，不管其他杂事）

---

## 常见问题

**Q: Kernel 和 AI 模型是什么关系？**
A: Kernel 是"大脑指挥官"，AI 模型是"思考引擎"。Kernel 负责组织信息、决定什么时候用工具、什么时候给用户回复，AI 模型负责真正的语言理解和生成。

**Q: 为什么需要多轮思考？**
A: 就像人思考问题一样，有时候需要查资料、计算、对比，AI 也需要多步推理才能给出完整答案。

**Q: 流式输出是什么意思？**
A: 就像看人打字一样，AI 生成一个字就显示一个字，不用等整段话都生成完。这样用户体验更好。

**Q: 工具执行失败怎么办？**
A: Kernel 会捕获错误信息，告诉 AI 模型工具失败了，AI 会根据情况重试或换一种方式回答。
