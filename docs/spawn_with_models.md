# Spawn with Model Selection

## Overview

The `spawn_parallel` tool now supports per-task model selection, enabling multi-model collaboration where different models handle different subtasks and results are aggregated.

## Usage

### Simple parallel tasks (same model)

```json
{
  "tasks": [
    "Analyze the performance bottlenecks",
    "Review security vulnerabilities",
    "Check code quality"
  ]
}
```

### Multi-model parallel tasks

```json
{
  "tasks": [
    {
      "task": "Quick code review for syntax errors",
      "model_id": "fast"
    },
    {
      "task": "Deep architectural analysis",
      "model_id": "reasoning"
    },
    {
      "task": "Generate comprehensive documentation",
      "model_id": "coder"
    }
  ]
}
```

## Configuration

Define model profiles in your config:

```yaml
agents:
  models:
    fast:
      provider: zhipu
      model: glm-4-flash
      description: "Fast responses for simple tasks"
      capabilities: ["fast", "simple"]
      temperature: 0.7

    reasoning:
      provider: anthropic
      model: claude-opus-4
      description: "Deep reasoning and analysis"
      capabilities: ["reasoning", "analysis"]
      temperature: 0.3

    coder:
      provider: openai
      model: gpt-4o
      description: "Code generation expert"
      capabilities: ["code", "documentation"]
      temperature: 0.2
```

### ModelProfile 字段说明

| 字段 | 类型 | 说明 |
|------|------|------|
| `provider` | string | 提供商 ID (openai, anthropic, zhipu 等) |
| `model` | string | 模型 ID (gpt-4o, claude-opus-4 等) |
| `description` | string | 模型描述 |
| `capabilities` | string[] | 能力标签 (code, reasoning, fast, creative 等) |
| `temperature` | float | 采样温度 (0.0-2.0) |
| `thinking_enabled` | bool | 是否启用推理模式 |
| `max_tokens` | int | 最大生成 token 数 |

## Benefits

1. **Cost optimization**: Use cheaper/faster models for simple tasks
2. **Quality optimization**: Use specialized models for complex tasks
3. **Parallel execution**: All tasks run concurrently
4. **Result aggregation**: Main agent receives all results for synthesis

## Example Workflow

```
Main Agent (Sonnet 4)
  ├─> Subagent 1 (Haiku) - Quick data extraction
  ├─> Subagent 2 (Sonnet) - Deep analysis
  └─> Subagent 3 (GPT-4) - Code generation

Main Agent synthesizes all results
```
