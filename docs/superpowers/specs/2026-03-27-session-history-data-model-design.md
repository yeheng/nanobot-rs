# Session/History 数据模型重设计

> 事件溯源架构设计规范

**版本**: 2.0
**日期**: 2026-03-27
**状态**: 设计阶段

---

## 1. 概述

### 1.1 设计目标

重新设计 Session/History 数据模型，解决以下问题：

| 问题 | 当前状态 | 目标状态 |
|------|----------|----------|
| Session 结构过于简单 | 仅支持线性消息列表 | 支持分支、版本控制、层级摘要 |
| AgentContext 抽象不合理 | `Arc<dyn Trait>` 动态分发 | `enum` 静态分发，零运行时开销 |
| 持久化策略不完善 | Hook 后保存，可能丢失数据 | 关键数据优先落盘 |
| Embedding 集成割裂 | 独立的 session_embeddings 表 | 每消息 Embedding，紧密集成 |
| 并发控制复杂 | `DashMap<AtomicBool>` | Actor-based 天然串行化 |

### 1.2 核心设计原则

1. **事件溯源 (Event Sourcing)**: 每条消息是不可变事件，会话状态是事件流的投影
2. **Enum 替代 Trait**: 使用 `enum AgentContext` 消除动态分发
3. **Actor-based 压缩**: 单一 CompressionActor 通过 channel 串行处理任务
4. **每消息 Embedding**: Embedding 作为事件元数据，支持语义检索

---

## 2. 核心数据结构

### 2.1 SessionEvent (核心实体)

```rust
use uuid::Uuid;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 会话事件 - 不可变的事实记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    /// 事件唯一标识 (UUID v7 时间有序)
    pub id: Uuid,

    /// 所属会话
    pub session_key: String,

    /// 父事件 ID (支持分支和版本控制)
    pub parent_id: Option<Uuid>,

    /// 事件类型
    pub event_type: EventType,

    /// 消息内容
    pub content: String,

    /// 语义向量 (每消息 Embedding)
    pub embedding: Option<Vec<f32>>,

    /// 事件元数据
    pub metadata: EventMetadata,

    /// 创建时间
    pub created_at: DateTime<Utc>,
}

/// 事件类型枚举
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventType {
    /// 用户消息
    UserMessage,

    /// 助手回复
    AssistantMessage,

    /// 工具调用
    ToolCall {
        tool_name: String,
        arguments: serde_json::Value,
    },

    /// 工具结果
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        is_error: bool,
    },

    /// 摘要事件 (压缩生成)
    Summary {
        summary_type: SummaryType,
        covered_event_ids: Vec<Uuid>,
    },

    /// 分支合并
    Merge {
        source_branch: String,
        source_head: Uuid,
    },
}

impl EventType {
    /// 获取摘要类型列表
    pub fn category_summary() -> &'static [EventType] {
        &[EventType::Summary { summary_type: SummaryType::Compression { token_budget: 0 }, covered_event_ids: vec![] }]
    }
}

/// 摘要类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SummaryType {
    /// 时间窗口摘要
    TimeWindow { duration_hours: u32 },

    /// 主题摘要
    Topic { topic: String },

    /// 压缩摘要 (超出 token 预算时)
    Compression { token_budget: usize },
}

/// 事件元数据
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventMetadata {
    /// 分支名称 (None 表示主分支)
    pub branch: Option<String>,

    /// 使用的工具列表
    pub tools_used: Vec<String>,

    /// Token 统计
    pub token_usage: Option<TokenUsage>,

    /// 扩展字段
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Token 使用统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}
```

### 2.2 Session (聚合根)

```rust
use std::collections::HashMap;

/// 会话 - 事件的聚合根
#[derive(Debug, Clone)]
pub struct Session {
    /// 会话标识
    pub key: String,

    /// 当前活跃分支
    pub current_branch: String,

    /// 所有分支指针 (branch_name -> latest_event_id)
    pub branches: HashMap<String, Uuid>,

    /// 会话元数据
    pub metadata: SessionMetadata,
}

/// 会话元数据
#[derive(Debug, Clone, Default)]
pub struct SessionMetadata {
    /// 创建时间
    pub created_at: DateTime<Utc>,

    /// 最后更新时间
    pub updated_at: DateTime<Utc>,

    /// 最后压缩点 (事件 ID)
    pub last_consolidated_event: Option<Uuid>,

    /// 总消息数
    pub total_events: usize,

    /// 累计 token 使用
    pub total_tokens: u64,
}

impl Session {
    /// 创建新会话
    pub fn new(key: impl Into<String>) -> Self {
        let key = key.into();
        let now = Utc::now();
        Self {
            key,
            current_branch: "main".to_string(),
            branches: HashMap::new(),
            metadata: SessionMetadata {
                created_at: now,
                updated_at: now,
                ..Default::default()
            },
        }
    }

    /// 从 SessionKey 创建
    pub fn from_key(key: SessionKey) -> Self {
        Self::new(key.to_string())
    }
}
```

### 2.3 AgentContext Enum

```rust
/// Agent 上下文 - 使用 Enum 替代 trait 动态分发
#[derive(Debug)]
pub enum AgentContext {
    /// 持久化上下文 (主 Agent)
    Persistent(PersistentContext),

    /// 无状态上下文 (子 Agent)
    Stateless,
}

/// 持久化上下文数据
#[derive(Debug)]
pub struct PersistentContext {
    /// 会话管理器
    session_manager: Arc<SessionManager>,

    /// 事件存储
    event_store: Arc<EventStore>,

    /// 历史检索器
    history_retriever: Arc<HistoryRetriever>,

    /// Embedding 服务
    embedding_service: Arc<EmbeddingService>,

    /// 压缩任务发送端
    compression_tx: mpsc::Sender<CompressionTask>,
}
```

---

## 3. 存储层设计

### 3.1 数据库 Schema

```sql
-- 会话元数据表
CREATE TABLE sessions (
    key             TEXT PRIMARY KEY,
    current_branch  TEXT NOT NULL DEFAULT 'main',
    branches        TEXT NOT NULL DEFAULT '{}',  -- JSON: {"main": "uuid", "explore": "uuid"}
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    last_consolidated_event TEXT,  -- UUID
    total_events    INTEGER NOT NULL DEFAULT 0,
    total_tokens    INTEGER NOT NULL DEFAULT 0
);

-- 事件表 (核心表)
CREATE TABLE session_events (
    id              TEXT PRIMARY KEY,           -- UUID v7
    session_key     TEXT NOT NULL,
    parent_id       TEXT,                       -- 父事件 UUID
    event_type      TEXT NOT NULL,              -- 'user'|'assistant'|'tool_call'|'tool_result'|'summary'|'merge'
    content         TEXT NOT NULL,
    embedding       BLOB,                       -- f32 vector (bytemuck)
    branch          TEXT DEFAULT 'main',
    tools_used      TEXT DEFAULT '[]',          -- JSON array
    token_usage     TEXT,                       -- JSON: {"input": 100, "output": 50}
    extra           TEXT DEFAULT '{}',          -- JSON
    created_at      TEXT NOT NULL,

    FOREIGN KEY (session_key) REFERENCES sessions(key) ON DELETE CASCADE
);

-- 索引
CREATE INDEX idx_events_session_branch ON session_events(session_key, branch);
CREATE INDEX idx_events_parent ON session_events(parent_id);
CREATE INDEX idx_events_created ON session_events(created_at);
CREATE INDEX idx_events_type ON session_events(event_type);

-- 摘要索引表 (加速多维度检索)
CREATE TABLE summary_index (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    session_key     TEXT NOT NULL,
    event_id        TEXT NOT NULL,              -- 摘要事件 ID
    summary_type    TEXT NOT NULL,              -- 'time_window'|'topic'|'compression'
    topic           TEXT,                       -- 主题摘要的主题
    covered_events  TEXT NOT NULL,              -- JSON: 覆盖的事件 ID 列表
    created_at      TEXT NOT NULL
);

CREATE INDEX idx_summary_session ON summary_index(session_key);
CREATE INDEX idx_summary_type ON summary_index(summary_type);
```

### 3.2 EventStore 接口

```rust
/// 事件存储 - 事件溯源的核心
pub struct EventStore {
    pool: SqlitePool,
}

impl EventStore {
    /// 创建新的事件存储
    pub async fn new(pool: SqlitePool) -> Result<Self, StoreError> {
        Ok(Self { pool })
    }

    /// 追加事件 (O(1) 操作)
    pub async fn append_event(&self, event: &SessionEvent) -> Result<(), StoreError>;

    /// 获取分支的历史事件流
    pub async fn get_branch_history(
        &self,
        session_key: &str,
        branch: &str,
    ) -> Result<Vec<SessionEvent>, StoreError>;

    /// 获取事件到指定点 (时间旅行)
    pub async fn get_events_up_to(
        &self,
        session_key: &str,
        branch: &str,
        target_event_id: Uuid,
    ) -> Result<Vec<SessionEvent>, StoreError>;

    /// 语义检索历史事件
    pub async fn search_by_embedding(
        &self,
        session_key: &str,
        query_embedding: &[f32],
        top_k: usize,
        event_types: Option<&[EventType]>,
    ) -> Result<Vec<(SessionEvent, f32)>, StoreError>;

    /// 按类型获取事件
    pub async fn get_events_by_type(
        &self,
        session_key: &str,
        branch: &str,
        event_types: &[EventType],
    ) -> Result<Vec<SessionEvent>, StoreError>;

    /// 创建分支
    pub async fn create_branch(
        &self,
        session_key: &str,
        branch_name: &str,
        from_event_id: Uuid,
    ) -> Result<(), StoreError>;

    /// 列出所有分支
    pub async fn list_branches(
        &self,
        session_key: &str,
    ) -> Result<Vec<BranchInfo>, StoreError>;
}
```

---

## 4. 压缩 Actor

### 4.1 CompressionActor

```rust
/// 压缩任务
#[derive(Debug)]
pub struct CompressionTask {
    pub session_key: String,
    pub branch: String,
    pub evicted_events: Vec<Uuid>,
    pub compression_type: SummaryType,
}

/// 压缩 Actor - 单线程处理所有压缩请求
pub struct CompressionActor {
    receiver: mpsc::Receiver<CompressionTask>,
    event_store: Arc<EventStore>,
    summarization: Arc<SummarizationService>,
    embedding_service: Arc<EmbeddingService>,
}

impl CompressionActor {
    /// 启动压缩 Actor，返回任务发送端
    pub fn spawn(
        event_store: Arc<EventStore>,
        summarization: Arc<SummarizationService>,
        embedding_service: Arc<EmbeddingService>,
    ) -> mpsc::Sender<CompressionTask> {
        let (tx, rx) = mpsc::channel(64);

        let actor = Self {
            receiver: rx,
            event_store,
            summarization,
            embedding_service,
        };

        tokio::spawn(async move {
            actor.run().await;
        });

        tx
    }

    async fn run(mut self) {
        while let Some(task) = self.receiver.recv().await {
            if let Err(e) = self.process_task(task).await {
                tracing::error!("Compression task failed: {}", e);
            }
        }
    }

    async fn process_task(&self, task: CompressionTask) -> Result<(), AgentError> {
        // 1. 加载被驱逐的事件
        let events = self.event_store
            .get_events_by_ids(&task.session_key, &task.evicted_events)
            .await?;

        // 2. 生成摘要
        let summary_content = self.summarization
            .summarize_events(&events)
            .await?;

        // 3. 生成摘要的 Embedding
        let summary_embedding = self.embedding_service
            .embed(&summary_content)
            .await?;

        // 4. 创建摘要事件
        let summary_event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: task.session_key,
            parent_id: events.last().map(|e| e.id),
            event_type: EventType::Summary {
                summary_type: task.compression_type,
                covered_event_ids: task.evicted_events,
            },
            content: summary_content,
            embedding: Some(summary_embedding),
            metadata: EventMetadata {
                branch: Some(task.branch),
                ..Default::default()
            },
            created_at: Utc::now(),
        };

        // 5. 持久化摘要事件
        self.event_store.append_event(&summary_event).await?;

        Ok(())
    }
}
```

---

## 5. 多维度历史检索

### 5.1 HistoryQuery

```rust
/// 历史检索器
pub struct HistoryRetriever {
    event_store: Arc<EventStore>,
    embedding_service: Arc<EmbeddingService>,
}

/// 检索查询条件
#[derive(Debug, Clone, Default)]
pub struct HistoryQuery {
    /// 会话标识
    pub session_key: String,

    /// 分支过滤 (None = 当前分支)
    pub branch: Option<String>,

    /// 时间范围
    pub time_range: Option<TimeRange>,

    /// 事件类型过滤
    pub event_types: Vec<EventType>,

    /// 语义搜索
    pub semantic_query: Option<SemanticQuery>,

    /// 工具使用过滤
    pub tools_filter: Vec<String>,

    /// 分页
    pub offset: usize,
    pub limit: usize,

    /// 排序
    pub order: QueryOrder,
}

#[derive(Debug, Clone)]
pub enum SemanticQuery {
    /// 文本查询 (自动生成 embedding)
    Text(String),
    /// 直接使用向量
    Embedding(Vec<f32>),
}

#[derive(Debug, Clone)]
pub enum QueryOrder {
    /// 时间正序
    Chronological,
    /// 时间倒序
    ReverseChronological,
    /// 相似度排序
    Similarity,
}

/// 检索结果
#[derive(Debug)]
pub struct HistoryResult {
    pub events: Vec<SessionEvent>,
    pub meta: ResultMeta,
}

#[derive(Debug, Default)]
pub struct ResultMeta {
    pub total_count: usize,
    pub has_more: bool,
    pub query_time_ms: u64,
}
```

### 5.2 检索 API

```rust
impl HistoryRetriever {
    /// 执行多维度查询
    pub async fn query(&self, query: HistoryQuery) -> Result<HistoryResult, AgentError>;

    /// 时间旅行 - 获取某个历史时刻的完整上下文
    pub async fn time_travel(
        &self,
        session_key: &str,
        branch: &str,
        target_event_id: Uuid,
    ) -> Result<Vec<SessionEvent>, AgentError>;

    /// 获取摘要链
    pub async fn get_summary_chain(
        &self,
        session_key: &str,
        branch: &str,
    ) -> Result<Vec<SessionEvent>, AgentError>;
}
```

---

## 6. AgentLoop 集成

### 6.1 新 AgentLoop 结构

```rust
/// Agent 执行循环
pub struct AgentLoop {
    /// 上下文 (Enum 替代 Arc<dyn Trait>)
    context: AgentContext,

    /// LLM 提供者
    provider: Arc<dyn LLMProvider>,

    /// 工具执行器
    tool_executor: ToolExecutor,

    /// Hook 注册表
    hooks: HookRegistry,

    /// 历史处理器
    history_processor: HistoryProcessor,

    /// 配置
    config: AgentConfig,
}
```

### 6.2 持久化策略

```rust
/// 持久化策略
pub enum PersistPolicy {
    /// 立即同步持久化，失败返回错误
    Immediate,
    /// 后台异步持久化，不阻塞
    Background,
    /// 异步尝试，失败不影响主流程
    AsyncBestEffort,
}

impl AgentLoop {
    /// 处理用户消息 (主入口)
    pub async fn process_message(
        &mut self,
        session_key: &SessionKey,
        user_input: &str,
        output_tx: mpsc::Sender<StreamEvent>,
    ) -> Result<(), AgentError> {
        // 1. 加载会话状态
        let session = self.context.load_session(session_key).await;

        // 2. 创建用户消息事件
        let user_event = self.create_user_event(
            session_key,
            user_input,
            &session.current_branch,
        ).await?;

        // ★ 关键：立即持久化用户消息 (在 Hook 之前)
        self.context.save_event(user_event.clone()).await?;

        // 3. 获取历史上下文
        let history = self.build_context(&session).await?;

        // 4. 调用 LLM (流式)
        let response = self.provider.chat_stream(request).await?;

        // 5. 处理流式响应
        let assistant_event = self.process_stream(...).await?;

        // ★ 关键：立即持久化助手消息 (在 Hook 之前)
        self.context.save_event(assistant_event.clone()).await?;

        // 6. 处理工具调用 (如果有)
        // ...

        // 7. 触发 AfterResponse Hook (在持久化之后)
        self.hooks.execute(HookPoint::AfterResponse, &session, &assistant_event).await;

        // 8. 检查是否需要压缩
        self.check_and_compress(&session).await?;

        Ok(())
    }
}
```

### 6.3 AgentContext 实现

```rust
impl AgentContext {
    /// 加载会话
    pub async fn load_session(&self, key: &SessionKey) -> Session {
        match self {
            Self::Persistent(ctx) => ctx.session_manager.get_or_create(key).await,
            Self::Stateless => Session::new(key),
        }
    }

    /// 保存事件
    pub async fn save_event(&self, event: SessionEvent) -> Result<(), AgentError> {
        match self {
            Self::Persistent(ctx) => {
                ctx.event_store.append_event(&event).await
                    .map_err(|e| AgentError::Persistence(e.to_string()))
            }
            Self::Stateless => Ok(()),
        }
    }

    /// 获取历史
    pub async fn get_history(&self, key: &str, branch: Option<&str>) -> Vec<SessionEvent> {
        match self {
            Self::Persistent(ctx) => {
                ctx.event_store.get_branch_history(key, branch.unwrap_or("main")).await
                    .unwrap_or_default()
            }
            Self::Stateless => vec![],
        }
    }

    /// 语义检索
    pub async fn search_relevant(
        &self,
        key: &str,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<SessionEvent>, AgentError> {
        match self {
            Self::Persistent(ctx) => {
                let embedding = ctx.embedding_service.embed(query).await?;
                let results = ctx.event_store
                    .search_by_embedding(key, &embedding, top_k, None)
                    .await?
                    .into_iter().map(|(e, _)| e).collect();
                Ok(results)
            }
            Self::Stateless => Ok(vec![]),
        }
    }

    /// 是否持久化
    pub fn is_persistent(&self) -> bool {
        matches!(self, Self::Persistent(_))
    }
}
```

---

## 7. 分支与版本控制

### 7.1 分支 API

```rust
/// 分支信息
#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub name: String,
    pub head_event_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub event_count: usize,
}

impl AgentContext {
    /// 创建新分支
    pub async fn create_branch(
        &self,
        session_key: &str,
        branch_name: &str,
        from_event_id: Uuid,
    ) -> Result<(), AgentError>;

    /// 切换分支
    pub async fn switch_branch(
        &self,
        session_key: &str,
        branch_name: &str,
    ) -> Result<Session, AgentError>;

    /// 列出所有分支
    pub async fn list_branches(&self, session_key: &str) -> Result<Vec<BranchInfo>, AgentError>;

    /// 合并分支
    pub async fn merge_branch(
        &self,
        session_key: &str,
        source_branch: &str,
        target_branch: &str,
    ) -> Result<(), AgentError>;
}
```

### 7.2 时间旅行

```rust
impl HistoryRetriever {
    /// 时间旅行 - 回到某个历史时刻
    ///
    /// 返回从根事件到目标事件的所有事件 (按时间顺序)
    pub async fn time_travel(
        &self,
        session_key: &str,
        branch: &str,
        target_event_id: Uuid,
    ) -> Result<Vec<SessionEvent>, AgentError> {
        self.event_store
            .get_events_up_to(session_key, branch, target_event_id)
            .await
            .map_err(Into::into)
    }
}
```

---

## 8. 需删除的旧代码

```
待删除文件/模块:
├── gasket/storage/src/session.rs         # 旧 SessionManager, SessionMessage
├── gasket/core/src/session/manager.rs    # 旧 SessionManager 引用
├── gasket/core/src/agent/context.rs      # AgentContext trait (用 enum 替代)
└── 相关测试文件

待修改文件:
├── gasket/core/src/agent/loop_.rs        # 使用新 AgentContext enum
├── gasket/core/src/agent/pipeline.rs     # 集成新 EventStore
├── gasket/storage/src/lib.rs             # 更新 Schema
└── 所有引用 SessionMessage 的模块
```

---

## 9. 实施阶段

| 阶段 | 内容 | 预计时间 |
|------|------|----------|
| 阶段 1 | 核心数据结构 (SessionEvent, EventType, AgentContext enum) | 2-3 天 |
| 阶段 2 | 存储层 (EventStore, Schema) | 3-4 天 |
| 阶段 3 | 检索系统 (HistoryRetriever, 多维度查询) | 2-3 天 |
| 阶段 4 | AgentLoop 集成 (持久化策略, Hook 调整) | 3-4 天 |
| 阶段 5 | 压缩 Actor (CompressionActor, 删除 DashMap) | 1-2 天 |
| 阶段 6 | 分支与版本控制 (分支 API, 时间旅行) | 2-3 天 |
| 阶段 7 | 清理与测试 (删除旧代码, 完整测试覆盖) | 2-3 天 |

**总计**: 15-20 天

---

## 10. 设计决策记录

| 决策 | 选择 | 理由 |
|------|------|------|
| 数据模型 | 事件溯源 | 天然支持分支、版本、多维度检索 |
| 抽象方式 | Enum 替代 Trait | 零运行时开销，编译时确定行为 |
| 并发控制 | Actor-based | 天然串行化，无锁，背压支持 |
| 迁移策略 | 直接删除旧数据 | 无技术债，简化实现 |
| Embedding | 每消息存储 | 紧密集成，支持语义检索 |
| 持久化 | 关键数据优先落盘 | 保证用户输入和 LLM 响应不丢失 |