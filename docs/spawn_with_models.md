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
      capabilities: ["quick", "simple"]
      temperature: 0.7

    reasoning:
      provider: anthropic
      model: claude-sonnet-4
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
