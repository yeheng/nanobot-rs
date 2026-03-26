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
    /// 检查是否为摘要类型事件
    pub fn is_summary(&self) -> bool {
        matches!(self, EventType::Summary { .. })
    }
}

/// 事件类型分类 (用于查询过滤)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventTypeCategory {
    UserMessage,
    AssistantMessage,
    ToolCall,
    ToolResult,
    Summary,
    Merge,
}

impl EventType {
    /// 获取事件类型分类
    pub fn category(&self) -> EventTypeCategory {
        match self {
            EventType::UserMessage => EventTypeCategory::UserMessage,
            EventType::AssistantMessage => EventTypeCategory::AssistantMessage,
            EventType::ToolCall { .. } => EventTypeCategory::ToolCall,
            EventType::ToolResult { .. } => EventTypeCategory::ToolResult,
            EventType::Summary { .. } => EventTypeCategory::Summary,
            EventType::Merge { .. } => EventTypeCategory::Merge,
        }
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
            branches: HashMap::new(), // 空映射表示尚无事件，主分支在首个事件追加时自动初始化
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

    /// 获取分支头事件 ID
    pub fn get_branch_head(&self, branch: &str) -> Option<Uuid> {
        self.branches.get(branch).copied()
    }

    /// 获取主分支头事件 ID
    pub fn main_head(&self) -> Option<Uuid> {
        self.get_branch_head("main")
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

impl PersistentContext {
    /// 创建持久化上下文
    ///
    /// 这会同时启动 CompressionActor 后台任务
    pub fn new(
        pool: SqlitePool,
        embedding_service: Arc<EmbeddingService>,
        summarization: Arc<SummarizationService>,
    ) -> Self {
        let event_store = Arc::new(EventStore::new(pool.clone()));
        let session_manager = Arc::new(SessionManager::new(pool));
        let history_retriever = Arc::new(HistoryRetriever::new(
            Arc::clone(&event_store),
            Arc::clone(&embedding_service),
        ));

        // 启动压缩 Actor
        let compression_tx = CompressionActor::spawn(
            Arc::clone(&event_store),
            summarization,
            Arc::clone(&embedding_service),
        );

        Self {
            session_manager,
            event_store,
            history_retriever,
            embedding_service,
            compression_tx,
        }
    }
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
    event_type      TEXT NOT NULL,              -- 事件类型: 'user_message'|'assistant_message'|'tool_call'|'tool_result'|'summary'|'merge'
    content         TEXT NOT NULL,
    embedding       BLOB,                       -- f32 vector (bytemuck)
    branch          TEXT DEFAULT 'main',
    tools_used      TEXT DEFAULT '[]',          -- JSON array
    token_usage     TEXT,                       -- JSON: {"input": 100, "output": 50}
    -- 事件类型特定字段 (用于存储 EventType 变体数据)
    tool_name       TEXT,                       -- ToolCall/ToolResult 的工具名称
    tool_arguments  TEXT,                       -- ToolCall 的参数 (JSON)
    tool_call_id    TEXT,                       -- ToolResult 的调用 ID
    is_error        INTEGER DEFAULT 0,          -- ToolResult 是否错误
    summary_type    TEXT,                       -- Summary 类型: 'time_window'|'topic'|'compression'
    summary_topic   TEXT,                       -- Topic 摘要的主题
    covered_events  TEXT,                       -- Summary 覆盖的事件 ID 列表 (JSON)
    merge_source    TEXT,                       -- Merge 的源分支名
    merge_head      TEXT,                       -- Merge 的源分支头事件 ID
    -- 通用字段
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
#[derive(Debug, Clone)]
pub struct CompressionTask {
    pub session_key: String,
    pub branch: String,
    pub evicted_events: Vec<Uuid>,
    pub compression_type: SummaryType,
    /// 重试次数
    pub retry_count: u32,
}

/// 压缩 Actor - 单线程处理所有压缩请求
pub struct CompressionActor {
    receiver: mpsc::Receiver<CompressionTask>,
    event_store: Arc<EventStore>,
    summarization: Arc<SummarizationService>,
    embedding_service: Arc<EmbeddingService>,
    /// 最大重试次数
    max_retries: u32,
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
            max_retries: 3,
        };

        tokio::spawn(async move {
            actor.run().await;
        });

        tx
    }

    async fn run(mut self) {
        while let Some(task) = self.receiver.recv().await {
            if let Err(e) = self.process_task(task.clone()).await {
                tracing::error!("Compression task failed: {}", e);

                // 重试机制
                if task.retry_count < self.max_retries {
                    let retry_task = CompressionTask {
                        retry_count: task.retry_count + 1,
                        ..task
                    };
                    tracing::warn!(
                        "Retrying compression task (attempt {}/{})",
                        retry_task.retry_count,
                        self.max_retries
                    );
                    // 指数退避重试
                    tokio::time::sleep(Duration::from_secs(2u64.pow(task.retry_count))).await;
                    if let Err(e) = self.process_task(retry_task).await {
                        tracing::error!("Compression retry failed: {}", e);
                    }
                } else {
                    tracing::error!(
                        "Compression task failed after {} retries, evicted events may be lost: {:?}",
                        self.max_retries,
                        task.evicted_events
                    );
                    // 可选：发送告警通知
                }
            }
        }
    }

    async fn process_task(&self, task: CompressionTask) -> Result<(), AgentError> {
        tracing::info!(
            "Processing compression for session '{}', {} events",
            task.session_key,
            task.evicted_events.len()
        );

        // 1. 加载被驱逐的事件
        let events = self.event_store
            .get_events_by_ids(&task.session_key, &task.evicted_events)
            .await?;

        if events.is_empty() {
            tracing::warn!("No events found for compression, skipping");
            return Ok(());
        }

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

        tracing::info!(
            "Compression complete: summary event {} created",
            summary_event.id
        );

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

impl HistoryQuery {
    /// 创建查询构造器
    pub fn builder(session_key: impl Into<String>) -> HistoryQueryBuilder {
        HistoryQueryBuilder::new(session_key)
    }
}

/// 查询构造器 (流式 API)
pub struct HistoryQueryBuilder {
    query: HistoryQuery,
}

impl HistoryQueryBuilder {
    pub fn new(session_key: impl Into<String>) -> Self {
        Self {
            query: HistoryQuery {
                session_key: session_key.into(),
                limit: 50,
                ..Default::default()
            },
        }
    }

    pub fn branch(mut self, branch: impl Into<String>) -> Self {
        self.query.branch = Some(branch.into());
        self
    }

    pub fn time_range(mut self, start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        self.query.time_range = Some(TimeRange { start, end });
        self
    }

    pub fn event_types(mut self, types: Vec<EventType>) -> Self {
        self.query.event_types = types;
        self
    }

    pub fn semantic_text(mut self, text: impl Into<String>) -> Self {
        self.query.semantic_query = Some(SemanticQuery::Text(text.into()));
        self
    }

    pub fn semantic_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.query.semantic_query = Some(SemanticQuery::Embedding(embedding));
        self
    }

    pub fn tools(mut self, tools: Vec<String>) -> Self {
        self.query.tools_filter = tools;
        self
    }

    pub fn limit(mut self, limit: usize) -> Self {
        self.query.limit = limit;
        self
    }

    pub fn offset(mut self, offset: usize) -> Self {
        self.query.offset = offset;
        self
    }

    pub fn order(mut self, order: QueryOrder) -> Self {
        self.query.order = order;
        self
    }

    pub fn build(self) -> HistoryQuery {
        self.query
    }
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

#### 6.2.1 Embedding 生成时机

```rust
/// Embedding 生成策略
#[derive(Debug, Clone)]
pub struct EmbeddingStrategy {
    /// 用户消息 Embedding
    pub user_messages: EmbeddingTiming,
    /// 助手消息 Embedding
    pub assistant_messages: EmbeddingTiming,
    /// 工具调用 Embedding
    pub tool_events: EmbeddingTiming,
}

#[derive(Debug, Clone)]
pub enum EmbeddingTiming {
    /// 同步生成 (阻塞，保证 Embedding 存在)
    Synchronous,
    /// 异步生成 (非阻塞，可能延迟)
    Asynchronous,
    /// 不生成
    Disabled,
}

impl Default for EmbeddingStrategy {
    fn default() -> Self {
        Self {
            user_messages: EmbeddingTiming::Asynchronous,
            assistant_messages: EmbeddingTiming::Asynchronous,
            tool_events: EmbeddingTiming::Disabled,
        }
    }
}
```

**Embedding 生成流程**:

1. **用户消息**: 默认异步生成，不阻塞主流程
   - 事件先持久化 (`embedding: None`)
   - 后台任务生成 Embedding 并更新

2. **助手消息**: 默认异步生成
   - 保证 LLM 响应快速返回
   - Embedding 用于后续语义检索

3. **Embedding 失败处理**:
   - 不影响消息持久化
   - 记录警告日志
   - 可通过后台任务重试

#### 6.2.2 消息持久化策略

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

/// 默认持久化策略配置
pub struct PersistConfig {
    /// 用户消息：立即持久化
    pub user_message: PersistPolicy::Immediate,
    /// 助手消息：立即持久化
    pub assistant_message: PersistPolicy::Immediate,
    /// 工具调用：立即持久化
    pub tool_call: PersistPolicy::Immediate,
    /// 摘要：后台持久化 (由 CompressionActor 处理)
    pub summary: PersistPolicy::Background,
    /// Embedding：异步生成
    pub embedding: PersistPolicy::AsyncBestEffort,
}
```

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

/// 合并策略
#[derive(Debug, Clone)]
pub enum MergeStrategy {
    /// Fast-forward: 如果目标分支没有新事件，直接移动指针
    FastForward,
    /// Merge commit: 创建 Merge 事件记录合并
    MergeCommit,
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
    ///
    /// # 合并语义
    ///
    /// 采用 **Merge Commit** 策略：
    /// 1. 在目标分支创建 `Merge` 事件
    /// 2. `parent_id` 指向目标分支最新事件
    /// 3. `Merge` 事件记录源分支名称和头事件 ID
    /// 4. 不修改历史事件（事件溯源不可变原则）
    ///
    /// # 冲突处理
    ///
    /// 由于事件不可变，不存在传统意义上的冲突。
    /// 合并后的历史将包含两个分支的所有事件，通过 `parent_id` 链追踪。
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

## 10. 测试策略

### 10.1 单元测试

| 测试类别 | 测试用例 | 重要性 |
|----------|----------|--------|
| 事件创建 | `SessionEvent` 序列化/反序列化 | 高 |
| 事件追加 | `EventStore::append_event` 正确存储 | 高 |
| 分支历史 | `get_branch_history` 返回正确事件链 | 高 |
| 分支创建 | `create_branch` 正确设置指针 | 高 |
| 语义检索 | `search_by_embedding` 返回相关结果 | 中 |
| 时间旅行 | `get_events_up_to` 正确截断历史 | 高 |

### 10.2 集成测试

| 测试场景 | 描述 | 重要性 |
|----------|------|--------|
| 完整对话流程 | 用户消息 → 助手响应 → 持久化 → 历史 | 高 |
| 分支切换 | 创建分支 → 切换 → 添加事件 → 切换回主分支 | 高 |
| 压缩流程 | 触发压缩 → 验证摘要事件创建 | 高 |
| 并发追加 | 多个事件同时追加到同一会话 | 中 |
| Embedding 生成 | 验证 Embedding 异步生成和存储 | 中 |

### 10.3 关键测试代码

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// 测试事件追加和读取
    #[tokio::test]
    async fn test_event_append_and_read() {
        let store = setup_test_store().await;

        let event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: "test:session".into(),
            parent_id: None,
            event_type: EventType::UserMessage,
            content: "Hello".into(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: Utc::now(),
        };

        store.append_event(&event).await.unwrap();

        let loaded = store.get_branch_history("test:session", "main").await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].content, "Hello");
    }

    /// 测试分支创建和历史隔离
    #[tokio::test]
    async fn test_branch_isolation() {
        let store = setup_test_store().await;

        // 主分支事件
        let e1 = create_test_event("test:session", None, "main");
        store.append_event(&e1).await.unwrap();

        let e2 = create_test_event("test:session", Some(e1.id), "main");
        store.append_event(&e2).await.unwrap();

        // 创建分支
        store.create_branch("test:session", "explore", e1.id).await.unwrap();

        // 分支事件
        let e3 = create_test_event("test:session", Some(e1.id), "explore");
        store.append_event(&e3).await.unwrap();

        // 验证隔离
        let main = store.get_branch_history("test:session", "main").await.unwrap();
        assert_eq!(main.len(), 2);

        let explore = store.get_branch_history("test:session", "explore").await.unwrap();
        assert_eq!(explore.len(), 2); // e1 + e3
    }

    /// 测试时间旅行
    #[tokio::test]
    async fn test_time_travel() {
        let store = setup_test_store().await;

        let e1 = create_test_event("test:session", None, "main");
        store.append_event(&e1).await.unwrap();

        let e2 = create_test_event("test:session", Some(e1.id), "main");
        store.append_event(&e2).await.unwrap();

        let e3 = create_test_event("test:session", Some(e2.id), "main");
        store.append_event(&e3).await.unwrap();

        // 回到 e2
        let history = store.get_events_up_to("test:session", "main", e2.id).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].id, e1.id);
        assert_eq!(history[1].id, e2.id);
    }

    /// 测试压缩 Actor 重试机制
    #[tokio::test]
    async fn test_compression_retry() {
        let (store, tx) = setup_compression_actor_with_failures(2).await;

        let task = CompressionTask {
            session_key: "test:session".into(),
            branch: "main".into(),
            evicted_events: vec![Uuid::now_v7()],
            compression_type: SummaryType::Compression { token_budget: 1000 },
            retry_count: 0,
        };

        tx.send(task).await.unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;

        // 验证最终成功
        let summaries = store.get_events_by_type("test:session", "main", &[EventTypeCategory::Summary]).await.unwrap();
        assert_eq!(summaries.len(), 1);
    }
}
```

### 10.4 属性测试

使用 `proptest` 进行属性测试：

```rust
proptest! {
    /// 测试事件链完整性
    #[test]
    fn test_event_chain_integrity(events in prop::collection::vec(any::<SessionEvent>(), 1..100)) {
        let mut prev_id: Option<Uuid> = None;
        for event in events {
            prop_assert_eq!(event.parent_id, prev_id);
            prev_id = Some(event.id);
        }
    }
}
```

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