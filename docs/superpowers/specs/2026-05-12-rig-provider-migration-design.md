# 用 rig-core 替代 gasket-provider 设计方案

## 背景与动机

`gasket-provider` 是 Gasket 框架的 LLM provider 抽象层，目前通过手写 `reqwest` HTTP 代码支持 7+ providers（OpenAI、Anthropic、Gemini、Copilot、MiniMax、Moonshot 等）。随着 provider 生态快速演进，维护手写适配层的成本持续上升。

`rig-core 0.36.0` 是一个成熟的 Rust LLM 框架，支持 25+ completion providers、10+ embedding providers、vector store 统一接口、agentic workflows 等能力。将 gasket 的 provider/embedding 层迁移到 rig，可以：

1. **消除 provider 维护负担**：rig 社区维护 25+ providers 的 HTTP 适配、SSE 解析、错误处理
2. **统一 embedding 接口**：rig 提供标准化的 `EmbeddingModel` trait，覆盖 OpenAI、Cohere、VoyageAI、Ollama 等
3. **打开 vector store 扩展性**：rig 的 `VectorStoreIndex` trait 支持 Qdrant、Pinecone 等外部后端，为未来架构演进留好接口

## 设计原则

- **保留现有类型作为防火墙**：`LlmProvider` trait、`ChatMessage`/`ChatRequest`/`ChatResponse` 等类型在第一阶段完全保留，改动限制在 `gasket-provider` 和 `gasket-embedding` 内部
- **分阶段交付**：P0 替换 completion provider → P1 替换 embedding provider → P2 vector store 可插拔设计
- **配置格式不背兼容包袱**：这是一次架构升级，配置格式按 rig 惯用法重新设计

---

## 阶段划分

| 阶段 | 范围 | 目标 | 预估工作量 |
|------|------|------|-----------|
| **P0** | `gasket-provider` 底层替换 | 删除所有手写 HTTP 代码，用 rig 的 completion providers 替代 | 1 周 |
| **P1** | `gasket-embedding` provider 层替换 | 用 rig 的 `EmbeddingModel` 替代 `ApiProvider`，`LocalOnnxProvider` 保留 | 3–5 天 |
| **P2** | Vector Store 可插拔设计 | 让 `VectorStore` trait 支持 rig `VectorStoreIndex` 作为可选后端 | 1 周 |
| **P3** | 配置迁移 + 集成测试 | 更新 `config.example.yaml`，全链路测试 | 3–5 天 |

---

## 整体架构

```
【现状】                          【目标（第一阶段）】
gasket-provider                   gasket-provider (保留 trait + 类型)
├─ LlmProvider trait              ├─ LlmProvider trait (保留)
├─ ChatMessage/ChatRequest        ├─ ChatMessage/ChatRequest (保留)
├─ hand-rolled reqwest            ├─ RigCompletionAdapter (新增)
│   ├─ OpenAICompatibleProvider   │   └─ 委托给 rig-core::CompletionModel
│   ├─ AnthropicProvider          │
│   ├─ GeminiProvider             │
│   └─ ... (7+ providers)         │
                                  │
gasket-embedding                  gasket-embedding (保留公共接口)
├─ EmbeddingProvider trait        ├─ EmbeddingProvider trait (保留)
├─ VectorStore trait              ├─ VectorStore trait (保留)
├─ ApiProvider (reqwest)          ├─ RigEmbeddingAdapter (新增)
├─ LocalOnnxProvider              ├─ LocalOnnxProvider (保留)
├─ EmbeddingStore (SQLite)        ├─ EmbeddingStore (保留)
└─ LanceVectorStore               └─ LanceVectorStore (保留)
                                  └─ RigVectorStoreAdapter (新增，可选)
                                  │
gasket-storage                    gasket-storage
├─ event_embeddings (SQLite)      ├─ event_embeddings (保留)
├─ WikiVectorStore adapter        ├─ WikiVectorStore adapter (保留)
└─ TantivyPageIndex (BM25)        └─ TantivyPageIndex (保留)
```

---

## P0: Completion Provider 替换

### 核心问题与解决方案

`rig::completion::CompletionModel` 是带有 associated types 的泛型 trait，不能直接 `Box<dyn CompletionModel>`。但 `gasket-engine` 通过 `Arc<dyn LlmProvider>` 使用 provider，因此适配器必须是具体类型。

**解决方案**：保留 `OpenAICompatibleProvider`、`AnthropicProvider` 等结构体名称和 `LlmProvider` 实现，但**内部完全删除手写 HTTP 代码**，改用 rig 的 provider client。

```rust
// 之前：内部是手写的 reqwest HTTP 客户端
pub struct OpenAICompatibleProvider {
    name: String,
    config: ProviderConfig,
    client: reqwest::Client,  // ← 删除
}

// 之后：内部委托给 rig 的 client
pub struct OpenAICompatibleProvider {
    name: String,
    config: ProviderConfig,
    rig_client: rig::providers::openai::Client,  // ← 新增
}
```

### 新增共享转换模块 `rig_bridge.rs`

所有 provider 共享同一套 **gasket 类型 ↔ rig 类型** 的转换逻辑：

| 转换方向 | 说明 |
|---------|------|
| `ChatRequest` → `CompletionRequest` | 将 `Vec<ChatMessage>` 转为 `OneOrMany<Message>`，提取 system message 为 `preamble` |
| `ToolDefinition` → `rig::ToolDefinition` | 去掉 `tool_type: "function"` 包装，直接取 `function.name/description/parameters` |
| `CompletionResponse` → `ChatResponse` | 从 `OneOrMany<AssistantContent>` 中提取 text 或 tool calls |
| `StreamedAssistantContent` → `ChatStreamChunk` | 将 rig 的流式事件映射为 gasket 的 `ChatStreamDelta` |
| `ProviderError` ↔ `CompletionError` | 错误类型映射（网络、速率限制、鉴权等） |

### 流式适配映射

rig 的 `StreamingCompletionResponse` 实现了 `Stream<Item = Result<StreamedAssistantContent<R>, CompletionError>>`：

| rig 事件 | gasket 映射 |
|---------|------------|
| `Text(Text)` | `ChatStreamDelta { content: Some(...), ... }` |
| `ToolCallDelta { content: Name(name) }` | `ToolCallDelta { function_name: Some(name), ... }` |
| `ToolCallDelta { content: Delta(delta) }` | `ToolCallDelta { function_arguments: Some(delta), ... }` |
| `ReasoningDelta { reasoning }` | `ChatStreamDelta { reasoning_content: Some(...), ... }` |
| `Final(R)` | 提取 usage，设置 `finish_reason` |

### 删除的代码

| 文件/模块 | 当前内容 | 替换后 |
|----------|---------|--------|
| `common.rs` | `OpenAICompatibleProvider` 的 HTTP 实现 | 委托给 `rig::providers::openai::Client` |
| `streaming.rs` | SSE 解析逻辑 | 删除，使用 rig 的流式响应 |
| `anthropic.rs` | Anthropic 专用 HTTP + SSE | 委托给 `rig::providers::anthropic::Client` |
| `gemini.rs` | Gemini 专用 HTTP + SSE | 委托给 `rig::providers::gemini::Client` |
| `copilot.rs` / `copilot_oauth.rs` | Copilot OAuth device flow + HTTP | 委托给 `rig::providers::copilot::Client`（rig 完整支持 OAuth device flow） |
| `minimax.rs` | MiniMax 专用 HTTP | 委托给 `rig::providers::minimax::Client` |
| `moonshot.rs` | Moonshot 专用 HTTP | 委托给 `rig::providers::moonshot::Client` |

### ProviderRegistry 变更

`engine/src/config/app_config.rs` 中的 `ProviderRegistry` 当前硬编码返回 `OpenAICompatibleProvider`。迁移后需要：

1. 根据配置中的 `provider_type` 创建对应的 rig client（openai、anthropic、gemini 等）
2. 用 `ModelSpec`（如 `"openai/gpt-4o"`）解析 provider 名称和 model ID
3. `ProviderRegistry::get_or_create()` 返回的仍是 `Arc<dyn LlmProvider>`，但内部实例已改为基于 rig 的适配器

### 新增 provider 能力

rig 支持而 gasket 当前未支持的 provider（迁移后自动获得）：
- Cohere、Perplexity、xAI、DeepSeek、Azure OpenAI、Mistral、Groq、Ollama、OpenRouter、Together、VoyageAI、Hyperbolic、HuggingFace 等

---

## P1: Embedding Provider 替换

### `ApiProvider` → `RigEmbeddingAdapter`

rig 的 `EmbeddingModel` 与 gasket 的 `EmbeddingProvider` 语义几乎一致：

```rust
// rig
pub trait EmbeddingModel: Send + Sync {
    fn ndims(&self) -> usize;
    fn embed_texts(&self, texts: impl IntoIterator<Item = String> + Send)
        -> impl Future<Output = Result<Vec<Embedding>, EmbeddingError>> + Send;
}

// gasket
pub trait EmbeddingProvider: Send + Sync {
    fn dim(&self) -> usize;
    async fn embed_batch(&self, texts: Vec<&str>) -> Result<Vec<Vec<f32>>, EmbeddingError>;
}
```

**唯一差异**：rig 返回 `Vec<f64>`，gasket 使用 `Vec<f32>`。在适配层做 `f64→f32` 转换。

### `LocalOnnxProvider` → 保留

`fastembed` 本地推理不在 rig 的能力范围内，保留原实现。

### 配置变更

```yaml
# 之前
embedding:
  provider: api
  api_endpoint: "https://api.openai.com/v1/embeddings"
  model: text-embedding-3-small
  api_key: "..."
  dim: 1536

# 之后（rig 风格）
embedding:
  provider: openai
  model: text-embedding-3-small
  api_key: "..."
  # dim 从 rig 的 EmbeddingModel 自动获取，无需配置
```

---

## P2: Vector Store 可插拔设计

### 设计决策：保留 `VectorStore` trait，新增 rig 后端选项

`gasket-embedding` 中的 `VectorStore` trait 已被 `RecallSearcher`、`EmbeddingIndexer`、`WikiVectorStore` 广泛使用。直接替换为 rig 的 `VectorStoreIndex` 会波及 engine 和 storage。

**更务实的做法**：
1. 保留 gasket 的 `VectorStore` trait 不变
2. 新增 `RigVectorStoreBackend` 作为 `VectorStore` 的一种**可选后端实现**
3. 内部持有实现了 rig `VectorStoreIndex` + `InsertDocuments` 的实例

```rust
pub struct RigVectorStoreBackend<I: rig::vector_store::VectorStoreIndex> {
    index: I,
    // embedding_model 用于在 search 时将文本查询转为向量
    //（如果 index 本身不处理 embedding）
    embedding_model: Arc<dyn EmbeddingProvider>,
}

impl<I: VectorStoreIndex + InsertDocuments> VectorStore for RigVectorStoreBackend<I> {
    async fn search(&self, query: &[f32], top_k: usize, min_score: f32) -> Result<Vec<SearchResult>> {
        // f32 query → rig Embedding → VectorSearchRequest → top_n_ids() → SearchResult
    }
    async fn upsert(&self, records: Vec<VectorRecord>) -> Result<()> {
        // VectorRecord 已包含预计算好的向量，直接转为 rig 文档格式
        // 调用 index.insert_documents() 写入
    }
}
```

### 默认策略

| 场景 | 推荐后端 |
|------|---------|
| 本地/边缘部署 | `EmbeddingStore`（SQLite，零依赖） |
| 需要 ANN 加速 | `LanceVectorStore`（已有） |
| 未来扩展 | `RigVectorStoreBackend<QdrantIndex>` 或 `RigVectorStoreBackend<PineconeIndex>` |

**第一阶段不强制引入 `RigVectorStoreBackend`**，但保留接口位置，为后续阶段做准备。

---

## 配置格式变更总览

### Provider 配置

```yaml
# 之前
providers:
  openai:
    api_base: "https://api.openai.com/v1"
    api_key: "..."
    default_model: "gpt-4o"
    extra_headers: {}
    proxy_url: null

# 之后（rig 风格，provider 由 rig 统一管理）
providers:
  openai:
    api_key: "..."
    base_url: "https://api.openai.com/v1"  # 可选，非官方端点时配置
    # default_model 在调用点指定，不再全局绑定到 provider
```

### 模型指定方式

保留 `ModelSpec` 的 `"provider/model"` 格式（如 `"openai/gpt-4o"`），但解析逻辑改为从 rig 的对应 provider client 获取 completion model。

---

## 风险与回滚策略

| 风险 | 缓解措施 |
|------|---------|
| rig 的流式行为与现有实现差异 | P0 阶段重点测试 streaming + tool call delta 的累积逻辑 |
| rig 的 provider 有 bug 或不支持某些参数 | 每个 rig provider 通过独立 feature flag 引入（如 `rig-openai`、`rig-anthropic`），如果某个 provider 行为异常，可在 `Cargo.toml` 中临时禁用对应 feature，回退到旧实现 |
| 配置格式变更导致用户困惑 | 提供详细的迁移指南和错误提示 |
| Copilot OAuth 行为差异 | rig 的 Copilot OAuth 实现完整度高于现有代码，但需验证 token 缓存路径 |

---

## 后续评估项（第二阶段）

1. **类型系统迁移**：评估将 engine 内部的 `ChatMessage`/`ChatRequest`/`ToolDefinition` 逐步迁移到 rig 的对应类型
2. **Agent 抽象**：评估用 rig 的 `Agent` 重构 engine 的 kernel/session 循环
3. **外部 Vector Store**：评估引入 Qdrant/Pinecone 作为可选后端

---

## 关键洞察

- **保留结构体壳是控制改动范围的关键**。`LlmProvider` trait 是 engine 的 15+ 处调用点的公共接口。保留壳 + 替换核，可以把改动限制在 `gasket-provider` 内部，避免同时修改 kernel、session、tools、hooks、subagents。
- **rig 的 `CompletionModel` associated types 不是问题**。虽然 `CompletionModel` 不能 dyn，但每个具体 provider（如 `openai::CompletionModel`）都是已知类型。适配器是具体结构体，内部持有具体的 rig client，实现 `LlmProvider` trait 时被 engine 当作 `dyn LlmProvider` 使用。
- **向量存储的"可插拔"设计不等于立刻替换 SQLite**。rig 的 `VectorStoreIndex` trait 是一个抽象边界。让 `gasket-storage` 实现这个 trait（以 SQLite 为默认后端），既能保持本地零依赖的优势，又为未来引入外部后端留下了单点切换的入口。
