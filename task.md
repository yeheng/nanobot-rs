
### 【品味评分】
**凑合 (Passable)** 

它没有变成一坨不可救药的垃圾，主要是因为你做了正确的事：把纯计算（`assemble_prompt`、`process_history`）和 LLM 核心循环（`AgentExecutor`）从编排流水线（`AgentLoop`）里剥离了出去。流水线就是一条直直的线，这一点很有品味。

但是，你在状态管理和并发控制上，沾染了严重的**“面向对象（OOP）脑残病”**。

### 【致命问题】

**1. 无脑的动态分发 (`Arc<dyn AgentContext>`)：不要在 Rust 里写 Java**
你在文档注释里很得意地写道：*“Uses AgentContext trait for state management... This eliminates `Option<T>` checks in the core loop.”*
简直荒谬。为了干掉一个极其廉价、分支预测器能 100% 猜中的 `if let Some(ctx)`，你引入了 `Arc<dyn Trait>`。
你知道动态分发意味着什么吗？意味着你要把指针追溯到虚函数表（vtable），彻底破坏 CPU 的指令缓存和数据局部性。虽然在 LLM 的网络 I/O 延迟面前这点性能损耗微不足道，但这完全是**糟糕的数据结构品味**。在 Rust 中，如果你只有两种已知状态（持久化和无状态），你永远应该用 `enum Context { Persistent(...), Stateless }` 或者干脆用 `Option<PersistentState>`。

**2. 简陋的并发补丁（背景压缩锁）**
看一眼 `PersistentContext::compress_context`：你用一个 `DashMap<String, Arc<AtomicBool>>` 来防止同一个 session 发生并发压缩。这是典型的发现并发问题后随手贴的创可贴。
这个操作极度丑陋。如果两个请求同时触发淘汰（eviction），第二个请求会默默失败或者跳过。更好的做法是让 SessionActor 自己拥有一个串行的压缩队列，或者把淘汰的消息压入一个基于 channel 的后台 Worker。

**3. 遗留代码的技术债（Streaming 流水线）**
你保留了老旧的基于闭包回调的 `process_direct_streaming<F>`，同时又写了一个基于 Channel 的 `process_direct_streaming_with_channel`。
"为了向后兼容"？那是放屁。这是内部核心 API，不是公开给第三方的 SDK。保留两套平行的代码只会让调用者迷惑，增加维护成本。

---

### 【关键洞察】

- **数据结构**：`HookRegistry` 和 `AgentContext` 大量滥用 `Arc<dyn Trait>`。系统根本不需要如此“极度开放”的扩展性。将多态收敛为静态类型（Enums 或 Generics）会使内存布局更紧凑。
- **复杂度**：`AgentLoop` 的线性流程很棒。但 Hook 系统的 `MutableContext` 和 `ReadonlyContext` 引入了泛型生命周期噩梦。虽然安全，但略显冗余。
- **风险点**：在 `process_direct` 的第 13 步，你是在所有的 Hook（包括可能有延迟的外部 Hook）执行完后才去 `save_message` (存助手回复)。如果系统在 Hook 执行时崩溃，LLM 消耗了 Token 生成的回复就会彻底丢失，破坏用户的连续性体验。

---

### 【Linus式方案】

如果值得做：
1. **干掉 `dyn AgentContext`**：用最笨但最清晰的 `enum AgentMode { Persistent(Storage), Stateless }` 替换它。用 `match` 来分发。
2. **重构内存压缩流**：不要用 `DashMap<AtomicBool>` 做并发控制。给 `SessionManager` 加一个异步的 `Sender<CompressionTask>`，单线程去处理 SQLite 写入和摘要调用。
3. **关键数据优先落盘**：拿到 LLM 的 `AgentResponse` 后，**第一时间**将其持久化到 SQLite，然后再丢给 `AfterResponse` Hook 去做那些无关紧要的审计和通知。
4. **清理僵尸代码**：直接删掉 `process_direct_streaming<F>` 及其附属逻辑。

---

### Task List

-[ ] **Task 1: 消除动态分发债务 (AgentContext)**
  - **What:** 将 `Arc<dyn AgentContext>` 替换为具体的 Enum 结构。
  - **Why:** 消除没必要的运行时虚表查找开销，符合 Rust 数据驱动的品味。
  - **Where:** `gasket/core/src/agent/context.rs` 和 `gasket/core/src/agent/loop_.rs`。
  - **How:** 
    1. 定义 `pub enum AgentContext { Persistent(PersistentContextData), Stateless }`。
    2. 为 Enum 实现具体的方法（如 `load_session`, `save_message`），内部使用 `match self` 分发。
    3. 移除 `async_trait` 宏。
  - **Test Case:** 跑通所有现有测试，确保 Subagent（Stateless）和 MainAgent（Persistent）行为不变。

- [ ] **Task 2: 修复状态持久化时机风险**
  - **What:** 在 `AgentLoop::process_direct` 中调整保存助手消息的时机。
  - **Why:** 必须保证核心状态优先落盘。如果 `AfterResponse` 外部 Shell Hook 阻塞或 panic，不能让昂贵的 LLM 响应数据丢失。
  - **Where:** `gasket/core/src/agent/loop_.rs` 的 `process_direct` 及 channel stream 方法。
  - **How:** 将 `self.context.save_message` 上移到 `self.hooks.execute(HookPoint::AfterResponse, ...)` 之前。
  - **Acceptance Criteria:** `AfterResponse` 返回 Abort 时，消息依然能够在 SQLite 历史中找到。

- [ ] **Task 3: 移除 Streaming 回调技术债**
  - **What:** 删除 `process_direct_streaming<F>` 方法。
  - **Why:** 你已经在用 `process_direct_streaming_with_channel` 处理带背压（backpressure）的流式输出了，旧代码是纯粹的垃圾。
  - **Where:** `gasket/core/src/agent/loop_.rs` 和调用它的 CLI 代码。
  - **How:** 移除旧函数，将 CLI 的老调用方改为 `process_direct_streaming_with_channel`，手动 `while let Some(event) = rx.recv().await` 处理输出。

- [ ] **Task 4: 重构背景压缩机制 (KISS原则)**
  - **What:** 废弃 `DashMap<String, Arc<AtomicBool>>` 的原子锁设计。
  - **Why:** 过度设计且容易引起逻辑上的状态泄漏（如果发生未捕获的错误导致锁未释放，会造成该 Session 永久无法压缩）。
  - **Where:** `PersistentContext::compress_context` (`gasket/core/src/agent/context.rs`)。
  - **How:** 将压缩请求丢入一个全局无界的 `mpsc::UnboundedSender<(String, Vec<SessionMessage>)>`，启动一个单一的后台 Actor 专门接收并处理这些淘汰请求。天然实现串行化，无锁。
  - **Acceptance Criteria:** 高频连续触发历史截断时，后台只有一个任务按顺序处理 Summarize 请求，不丢失数据，也不产生并发锁。