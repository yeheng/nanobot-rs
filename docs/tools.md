# 工具系统

> AI 的百宝箱——让 AI 能操作电脑、访问网络、管理文件

---

## 一句话理解

工具就是 AI 的**手和脚**，让 AI 不仅能"说话"，还能"做事"。

```mermaid
flowchart LR
    A[AI大脑] -->|需要查资料| B[网络搜索]
    A -->|需要读文件| C[文件操作]
    A -->|需要执行命令| D[命令执行]
    A -->|需要创建子任务| E[子代理]
    B --> A
    C --> A
    D --> A
    E --> A
```

---

## 为什么需要工具？

没有工具的 AI 就像一个**只有大脑、没有手脚**的人：

| 场景 | 没有工具 | 有工具 |
|------|---------|--------|
| 查今天天气 | "我无法获取实时信息" | [搜索网络] "今天北京25°C" |
| 分析你的代码 | "请把代码贴给我" | [读取文件] 直接分析项目 |
| 执行数据处理 | "请手动运行脚本" | [执行命令] 自动处理 |
| 获取最新新闻 | "我的知识截止到..." | [搜索网络] 获取实时新闻 |

```mermaid
flowchart TB
    subgraph 只有大脑
        A1[用户: 查天气]
        B1[AI: 对不起，我不知道]
    end
    
    subgraph 大脑+手脚
        A2[用户: 查天气]
        B2[AI: 稍等，我查一下]
        C2[调用天气工具]
        D2[获取数据]
        E2[AI: 今天25°C]
    end
    
    A1 --> B1
    A2 --> B2 --> C2 --> D2 --> E2
    
    style B1 fill:#FFCDD2
    style E2 fill:#C8E6C9
```

---

## 工具分类

```mermaid
mindmap
  root((AI工具箱))
    文件操作
      读取文件
      写入文件
      编辑文件
      列出目录
    网络访问
      网页搜索
      网页获取
    系统命令
      执行Shell
    记忆管理
      搜索记忆
      保存记忆
    任务管理
      创建子代理
      定时任务
    通信
      发送消息
```

---

## 各类工具详解

### 1. 文件操作工具

让 AI 能读写你电脑上的文件：

```mermaid
sequenceDiagram
    participant U as 用户
    participant AI as AI
    participant F as 文件工具
    participant Disk as 电脑硬盘
    
    U->>AI: 帮我看看main.py
    AI->>F: 读取main.py
    F->>Disk: 读取文件内容
    Disk-->>F: 返回代码
    F-->>AI: 代码内容
    AI-->>U: 这段代码的功能是...
    
    U->>AI: 修复第10行的bug
    AI->>F: 编辑文件，修改第10行
    F->>Disk: 写入修改
    F-->>AI: 修改成功
    AI-->>U: 已修复，把+改成-了
```

**包含的工具：**
- `read_file` - 读取文件内容
- `write_file` - 创建/覆盖文件
- `edit_file` - 修改文件部分内容
- `list_dir` - 查看文件夹内容

**使用场景：**
- 代码审查和修改
- 配置文件编辑
- 批量文件处理
- 项目结构分析

### 2. 网络工具

让 AI 能上网查资料：

```mermaid
sequenceDiagram
    participant U as 用户
    participant AI as AI
    participant W as 网络工具
    participant Net as 互联网
    
    U->>AI: 今天有什么AI新闻？
    
    Note over AI: AI没有2024年后的知识
    
    AI->>W: 搜索"AI新闻"
    W->>Net: 调用搜索引擎API
    Net-->>W: 返回搜索结果
    W-->>AI: 10条相关新闻
    
    AI->>W: 获取第一条详情
    W->>Net: 访问网页
    Net-->>W: 返回网页内容
    W-->>AI: 详细内容
    
    AI-->>U: 今天的主要AI新闻有...
```

**包含的工具：**
- `web_search` - 搜索引擎（Brave/Tavily/Exa）
- `web_fetch` - 获取特定网页内容

**使用场景：**
- 获取实时信息
- 查阅最新文档
- 研究某个话题
- 验证事实

### 3. Shell 执行工具

让 AI 能执行系统命令：

```mermaid
sequenceDiagram
    participant U as 用户
    participant AI as AI
    participant S as Shell工具
    participant OS as 操作系统
    
    U->>AI: 运行测试
    AI->>S: 执行"npm test"
    S->>OS: 创建子进程
    OS-->>S: 返回测试结果
    S-->>AI: 测试通过/失败详情
    AI-->>U: 测试已完成，3个通过，1个失败...
```

**包含的工具：**
- `exec` - 执行 Shell 命令（带安全限制）

**使用场景：**
- 运行测试
- 构建项目
- 执行数据处理脚本
- 系统管理任务

**安全保护：**
```mermaid
flowchart TB
    Command[用户命令] --> Check{安全检查}
    Check -->|危险命令| Block[拒绝执行]
    Check -->|允许命令| Run[执行]
    Check -->|需要确认| Confirm[询问用户]
    Confirm -->|同意| Run
    Confirm -->|拒绝| Block
    
    Block --> Error[返回错误]
    Run --> Result[返回结果]
    
    style Block fill:#FFCDD2
    style Run fill:#C8E6C9
```

### 4. 记忆工具

让 AI 能读写长期记忆：

```mermaid
sequenceDiagram
    participant U as 用户
    participant AI as AI
    participant M as 记忆工具
    participant Store as 记忆存储
    
    U->>AI: 我叫小明，做后端开发
    
    Note over AI: 判断这是重要信息
    
    AI->>M: 保存记忆 (type: note/skill)
    Note over M: 判断类型：事实→note，流程→skill
    M->>Store: 写入对应抽屉
    M-->>AI: 保存成功 (type: note)
    
    ...第二天...
    
    U->>AI: 帮我写段代码
    AI->>M: 搜索用户相关信息
    M->>Store: 查询记忆
    Store-->>M: 小明，后端开发
    M-->>AI: 用户信息
    
    Note over AI: 根据用户背景调整回答
    
    AI-->>U: 小明，这段后端代码...
```

**包含的工具：**
- `memory_search` - 搜索记忆
- `memorize` - 保存新记忆（支持 `memory_type`: `"note"` 或 `"skill"`）
- `memory_decay` - 清理旧记忆
- `memory_refresh` - 刷新记忆索引

### 5. 子代理工具

让 AI 能创建"分身"处理复杂任务：

```mermaid
sequenceDiagram
    participant U as 用户
    participant Main as 主AI
    participant Spawn as 子代理工具
    participant Sub as 子AI
    participant Res as 结果汇总
    
    U->>Main: 分析这个项目的所有文件
    
    Note over Main: 文件太多，需要并行处理
    
    Main->>Spawn: 创建子代理1<br/>分析前端代码
    Main->>Spawn: 创建子代理2<br/>分析后端代码
    Main->>Spawn: 创建子代理3<br/>分析测试文件
    
    Spawn->>Sub: 启动3个子AI
    
    par 并行执行
        Sub->>Sub: 分析前端
    and
        Sub->>Sub: 分析后端
    and
        Sub->>Sub: 分析测试
    end
    
    Sub-->>Res: 返回3份分析报告
    Res-->>Main: 汇总结果
    Main-->>U: 完整项目分析...
```

**包含的工具：**
- `spawn` - 创建单个子代理
- `spawn_parallel` - 并行创建多个子代理

### 6. 定时任务工具

让 AI 能管理定时任务：

```mermaid
sequenceDiagram
    participant U as 用户
    participant AI as AI
    participant C as Cron工具
    participant Cron as Cron服务
    
    U->>AI: 每天早上9点提醒我喝水
    
    AI->>C: 创建定时任务
    C->>Cron: 添加任务
    Cron-->>C: 添加成功
    C-->>AI: 任务ID: reminder-001
    
    Note over Cron: 每天早上9:00
    
    Cron->>AI: 触发任务
    AI-->>U: [消息] 早上好！记得喝水哦
```

**包含的工具：**
- `cron` - 管理定时任务（增删改查）

---

## 工具注册表

所有工具都登记在一个"工具箱"里：

```mermaid
flowchart TB
    subgraph 工具注册表
        R[工具箱]
        R --> T1[文件工具]
        R --> T2[网络工具]
        R --> T3[Shell工具]
        R --> T4[记忆工具]
        R --> T5[子代理工具]
        R --> T6[定时任务]
    end
    
    AI[AI大脑] --> R
    
    AI -->|需要查资料| T2
    AI -->|需要改文件| T1
    AI -->|需要执行命令| T3
```

### 语义路由

AI 会自动选择最合适的工具：

```mermaid
flowchart TB
    Q[用户问题] --> Understand[理解意图]
    
    Understand -->|查资料| Search[网络搜索]
    Understand -->|改代码| File[文件编辑]
    Understand -->|运行测试| Shell[命令执行]
    Understand -->|记住这个| Memory[保存记忆]
    Understand -->|任务太复杂| Subagent[创建子代理]
    
    style Understand fill:#FFD700
```

---

## 工具执行流程

```mermaid
sequenceDiagram
    participant AI as AI大脑
    participant K as Kernel
    participant R as 工具注册表
    participant T as 具体工具
    participant E as 外部环境
    
    AI->>K: 我需要搜索"天气"
    
    K->>R: 查找"web_search"工具
    R-->>K: 返回工具实例
    
    K->>T: 执行搜索<br/>参数: "天气"
    
    T->>E: 调用搜索引擎API
    E-->>T: 返回结果
    
    T-->>K: 整理结果
    K-->>AI: 搜索完成，结果是...
```

---

## 工具使用示例

### 示例1：复杂编程任务

```mermaid
flowchart TB
    Start([开始]) --> A[用户: 帮我优化这个Python项目]
    
    A --> B[AI: 读取项目结构]
    B --> C[list_dir]
    C --> D[发现main.py, utils.py, tests/]
    
    D --> E[AI: 读取main.py]
    E --> F[read_file]
    F --> G[分析代码]
    
    G --> H[AI: 搜索Python最佳实践]
    H --> I[web_search]
    I --> J[获取优化建议]
    
    J --> K[AI: 执行性能测试]
    K --> L[exec: python benchmark.py]
    L --> M[获取性能数据]
    
    M --> N[AI: 应用优化]
    N --> O[edit_file]
    O --> P[保存优化后的代码]
    
    P --> Q[AI: 运行测试]
    Q --> R[exec: pytest]
    R --> S[测试通过]
    
    S --> T([完成: 项目已优化])
    
    style Start fill:#90EE90
    style T fill:#C8E6C9
```

### 示例2：研究性任务

```mermaid
sequenceDiagram
    participant U as 用户
    participant AI as AI
    participant S as 搜索
    participant F as 网页获取
    participant Mem as 记忆
    
    U->>AI: 研究一下最新的AI趋势
    
    AI->>S: 搜索"2024 AI趋势"
    S-->>AI: 10篇文章链接
    
    AI->>F: 获取前3篇文章
    F-->>AI: 详细内容
    
    AI->>AI: 总结要点
    
    Note over AI: 重要信息，值得保存
    
    AI->>Mem: 保存记忆<br/>"2024 AI趋势: ..."
    
    AI-->>U: 根据最新资料，2024年AI趋势有...
```

---

## MCP 工具扩展

Gasket 还支持**外部工具服务**（MCP）：

```mermaid
flowchart TB
    subgraph Gasket
        AI[AI大脑]
        MCP[MCP客户端]
    end
    
    subgraph 外部工具服务
        S1[数据库查询服务]
        S2[图像生成服务]
        S3[代码分析服务]
        S4[企业API服务]
    end
    
    AI --> MCP
    MCP --> S1
    MCP --> S2
    MCP --> S3
    MCP --> S4
    
    S1 --> MCP
    S2 --> MCP
    S3 --> MCP
    S4 --> MCP
    MCP --> AI
```

**举例：**
- 连接公司内部的员工查询系统
- 连接专业的图像生成 AI
- 连接数据库执行 SQL 查询

---

## 常见问题

**Q: AI 能随便执行任何命令吗？**
A: 不能。有安全策略限制，危险命令会被拦截或需要用户确认。

**Q: AI 能访问我电脑上的所有文件吗？**
A: 只能访问工作空间内的文件，不会随意读取系统敏感文件。

**Q: 工具执行失败怎么办？**
A: AI 会收到错误信息，然后决定重试、换种方式、或告诉用户出错了。

**Q: 怎么知道 AI 用了什么工具？**
A: 在流式输出中可以看到工具调用信息，比如"正在搜索..."、"正在读取文件..."
