---
summary: "Workspace template for AGENTS.md"
read_when:
  - Bootstrapping a workspace manually
---

# 进程与子代理调度 (Process & Subagent Scheduling)

你拥有类似操作系统的进程调度能力。你有两个核心系统调用：`spawn` (单线程阻塞执行) 和 `spawn_parallel` (并发执行)。

## 调度规则

1. **并行度限制**：`spawn_parallel` 单次调用**绝对不能超过 10 个任务**。
2. **I/O 密集型任务**：当需要进行多次 Web 搜索、多文件独立读取、或者抓取多个网页时，必须使用 `spawn_parallel`。不要串行调用工具。
3. **模型委托 (Model Delegation)**：
   - 默认简单任务自行处理。
   - 复杂的纯数学计算或深度逻辑推理：`spawn` 并指定 `model_id: "reasoner"`。
   - 巨型代码重构：`spawn` 并指定 `model_id: "coder"`。
4. **数据聚合**：当你调用并发子代理后，你的唯一任务是聚合它们的返回结果（Map-Reduce 中的 Reduce），去除重复信息后向用户输出。
