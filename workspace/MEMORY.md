---
summary: "Gasket 记忆系统 — 跨会话长期记忆"
read_when:
  - 需要了解记忆系统工作原理
  - 手动初始化或维护记忆目录
---

# Gasket 记忆系统

基于 **场景 (Scenario) × 频率 (Frequency)** 二维模型的长期记忆存储与检索系统。

## 目录结构

```
~/.gasket/memory/
├── profile/          # 用户身份与偏好
├── active/           # 当前工作与焦点
├── knowledge/        # 学到的知识
├── decisions/        # 决策与理由
├── episodes/         # 经历与事件
└── reference/        # 外部参考资料
```

## 六大场景

| 场景 | 用途 | 加载时机 | Token 预算 |
|------|------|---------|-----------|
| **profile** | 用户身份、偏好、沟通风格 | 每次会话必加载 | ~200 |
| **active** | 当前工作焦点、待办事项 | 每次会话必加载 | ~500 |
| **knowledge** | 学到的概念、模式、约定 | 按主题匹配加载 | ~1000 |
| **decisions** | 做出的选择及其理由 | 决策场景或语义搜索 | ~1000 |
| **episodes** | 经历、事件及其结果 | 主要通过语义搜索 | 按需 |
| **reference** | 外部链接、联系人、工具 | 显式请求或语义搜索 | 按需 |

## 记忆文件格式

```markdown
---
id: mem_0192456c-1a2b-7def-8901-2b3c4d5e6f70
title: "选择 SQLite 作为主存储后端"
type: decision
scenario: decisions
tags: [gasket, database, sqlite, architecture]
frequency: warm
access_count: 12
created: 2026-04-01T10:00:00Z
updated: 2026-04-03T15:30:00Z
last_accessed: 2026-04-03T15:30:00Z
auto_expire: false
expires: null
tokens: 180
---

选择 SQLite 作为主存储后端的原因...
```

### 频率层级

| 层级 | 含义 | 加载策略 |
|------|------|---------|
| **hot** | 始终加载 | 场景激活时必定注入上下文 |
| **warm** | 按主题加载 | 标签匹配时注入上下文 |
| **cold** | 按需搜索 | 仅在显式搜索时加载 |
| **archived** | 历史归档 | 不主动加载，仅保留 |

## 核心特性

- **人类可编辑** — 每条记忆是独立的 `.md` 文件，任何文本编辑器可直接修改
- **懒加载** — 三阶段加载策略，硬性 Token 上限（3200 tokens）防止上下文爆炸
- **自动分层** — 根据访问频率自动调整加载优先级
- **语义搜索** — 嵌入向量搜索跨越场景边界连接相关记忆
- **版本历史** — 每次修改自动保存历史版本到 `.history/`
- **去重检测** — 定时扫描发现潜在重复记忆

## 初始化

```bash
# 手动创建记忆目录
mkdir -p ~/.gasket/memory/{profile,active,knowledge,decisions,episodes,reference}

# 或通过 Agent 首次写入记忆时自动创建
```