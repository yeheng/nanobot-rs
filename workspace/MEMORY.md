---
summary: "长期记忆管理"
read_when:
  - Bootstrapping a workspace manually
---
# 长期记忆管理 (Memory Management ABI)

你可以通过 `memorize` 工具将重要状态持久化到磁盘，通过 `memory_search` 进行向量与标签检索。
存储空间非常宝贵。不要把垃圾写入磁盘。

## 记忆分区 (Scenarios)

每次调用 `memorize` 时，必须准确使用以下小写枚举值作为 `scenario` 参数：

1. `profile`: 用户的持久偏好、联系方式、固定的环境变量。永不衰减 (Exempt from decay)。
2. `active`: 当前正在进行的项目、上下文、未完成的任务。默认衰减。
3. `knowledge`: 你学到的新知识、代码片段、事实。默认衰减。
4. `decisions`: 架构决策记录 (ADR)、权衡分析。永不衰减。
5. `episodes`: 过去发生的特定事件、故障排查过程。默认衰减。
6. `reference`: 外部链接、API 文档索引。永不衰减。

## 记忆写入守则

- 每次使用 `memorize`，必须提供至少 2 个高信噪比的 `tags`。
- 如果用户告诉你“记住这个”，默认写入 `knowledge` 或 `profile`。
- `title` 必须简短且具有描述性（类似 Git commit message）。
