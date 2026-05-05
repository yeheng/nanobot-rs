【核心判断】
值得做：方案 A（基于现有的 JSON-RPC 插件机制）是最有品味的解法。它把图灵完备的控制流（循环、分支）交给了本就擅长此事的 Python，保持了 Rust 核心引擎的极简和无状态。

【关键洞察】

- **目前的盲点**：目前 `SubagentSpawnHandler`（在 `gasket/engine/src/plugin/dispatcher/subagent.rs` 中）虽然允许插件生成子代理，但它是通过调用 `spawner.spawn().await` 同步等待结果的。**它吃掉了所有的流式事件（StreamEvents）！** 如果一个 Python 脚本耗时 3 分钟运行 4 个 subagent，用户界面会像死机了一样，直到最后才突然吐出结果。
- **核心修复**：必须把 `SubagentSpawnHandler` 改为调用 `spawn_with_stream`，并将事件转发到 `ctx.engine.outbound_tx`，就像原生的 `SpawnTool` 所做的那样。
- **零侵入**：一旦修好这个流式转发的管道，业务逻辑就完全是 Python 脚本的事了。

---

### Task List (方案 A：JSON-RPC 插件方案)

#### Task 1: 修复 RPC 子代理的流式事件转发 (Rust 核心)

- **What**: 修改 `SubagentSpawnHandler`，使其将子代理的实时流事件转发给 WebSocket/客户端。
- **Why**: 工作流执行耗时极长。如果不转发事件，用户在几分钟内看不到任何输出（没有 Thinking 和打字效果），体验极差。
- **Where**: `gasket/engine/src/plugin/dispatcher/subagent.rs`
- **How**:
  1. 将 `ctx.engine.spawner.spawn(...)` 替换为 `ctx.engine.spawner.spawn_with_stream(...)`。
  2. 拿到 `event_rx` 后，写一个 `while let Some(event) = event_rx.recv().await` 的循环。
  3. 利用 `event.to_chat_event_unconditional()`（或单独 match 转换），封装为 `OutboundMessage`，调用 `ctx.engine.outbound_tx.send(msg)` 发送给前端。
  4. 最后 `result_rx.await` 获取最终结果并返回 JSON。
- **Test Case**: 编写一个测试插件调用 `subagent/spawn`，断言 `outbound_tx` 收到了 `SubagentThinking` 和 `SubagentContent` 事件。
- **Acceptance Criteria**: 插件调用的 Subagent 也能在前端看到实时的打字机效果。

#### Task 2: 提供极简的 Python Gasket SDK (Python 基建)

- **What**: 编写一个 `gasket_sdk.py`，封装底层的 `sys.stdin.readline` 和 JSON-RPC 细节。
- **Why**: 避免在每个 workflow 脚本里写大量恶心、重复的 JSON 序列化和 ID 递增代码。
- **Where**: `workspace/plugins/gasket_sdk.py`
- **How**:
  实现一个轻量级的 `GasketPlugin` 类：
  - 维护一个递增的 `_request_id`。
  - 提供 `def spawn_subagent(self, task: str, model: str = None) -> str` 方法，内部发送 `{"method": "subagent/spawn", ...}` 并阻塞等待结果。
- **Test Case**: N/A（纯 Python 辅助脚本）
- **Acceptance Criteria**: 其他脚本可以通过 `from gasket_sdk import GasketPlugin` 优雅地调用引擎能力。

#### Task 3: 实现 Dev Workflow (Python 业务逻辑)

- **What**: 编写 Research-Plan-Implement-Review 工作流插件。
- **Why**: 实现需求，通过 Python 完成多节点 Prompt 定制和循环审核逻辑。
- **Where**:
  - 逻辑：`workspace/plugins/workflows/dev.py`
  - 声明：`workspace/plugins/dev_workflow.yaml`
- **How**:
  1. **YAML配置**: `protocol: json_rpc`, 声明 `permissions: [subagent_spawn]`，入参包含 `task`。
  2. **Python逻辑**:

     ```python
     plugin = GasketPlugin()
     task = plugin.get_args().get("task")
     
     # Research & Plan
     research = plugin.spawn_subagent(f"Research strictly on: {task}", model="reasoner")
     plan = plugin.spawn_subagent(f"Create steps based on research:\n{research}", model="reasoner")
     
     # Implement & Review Loop
     code = ""
     for i in range(3):
         code = plugin.spawn_subagent(f"Implement this plan:\n{plan}\nPrevious code (if any):\n{code}", model="coder")
         review = plugin.spawn_subagent(f"Review this code. If perfect, output 'PASS'. Else explain why:\n{code}", model="reasoner")
         
         if "PASS" in review:
             break
         plan = review # 用 review 意见覆盖 plan，继续循环
     
     plugin.return_result({"final_code": code})
     ```

- **Test Case**: 使用 CLI 触发：`gasket tool execute dev_workflow '{"task": "write a python snake game"}'`
- **Acceptance Criteria**: 工作流正常流转，触发多次 LLM 调用，当 review 不通过时成功触发循环，并在最后输出最终代码。
