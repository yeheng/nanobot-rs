# Spawn with Model Selection

> How to use different models for subagents

---

## Overview

When spawning subagents, you can specify different models than the main agent uses. This allows:

- Using cheaper models for simple tasks
- Using more powerful models for complex reasoning
- Optimizing cost and performance

---

## Basic Usage

### Using a Specific Model

```rust
// Spawn subagent with a specific model
let task = TaskSpec::new("sub-1", "Summarize this text")
    .with_model("gpt-4o-mini")  // Cheaper model for simple task
    .with_system_prompt("You are a summarizer".to_string());

let (result_tx, mut result_rx) = mpsc::channel(1);
spawn_subagent(
    provider,
    tools,
    workspace,
    task,
    None,
    result_tx,
    None,
);
let result = result_rx.recv().await;
```

### With Streaming

```rust
// Spawn with streaming and specific model
let task = TaskSpec::new("sub-1", "Analyze this code")
    .with_model("claude-4.5-sonnet")  // Better model for code
    .with_system_prompt("You are a code reviewer".to_string());

let (result_tx, mut result_rx) = mpsc::channel(1);
spawn_subagent(
    provider,
    tools,
    workspace,
    task,
    Some(event_tx),
    result_tx,
    None,
);
let result = result_rx.recv().await;
```

---

## Model Selection Guide

| Task Type | Recommended Model | Why |
|-----------|-------------------|-----|
| Simple extraction | gpt-4o-mini | Fast, cheap |
| Text summarization | gpt-4o-mini | Sufficient capability |
| Code review | claude-4.5-sonnet | Better at code |
| Creative writing | claude-4.5-sonnet | More creative |
| Data analysis | deepseek-chat | Good structured output |
| Complex reasoning | deepseek-reasoner/o1 | Better reasoning |
| Chinese tasks | glm-5/deepseek-chat | Optimized for Chinese |

---

## Cost Optimization

### Tiered Approach

```rust
// Use cheap model first, escalate if needed
let cheap_task = TaskSpec::new("sub-cheap", task.clone())
    .with_model("gpt-4o-mini");

let (tx1, mut rx1) = mpsc::channel(1);
spawn_subagent(
    provider.clone(),
    tools.clone(),
    workspace.clone(),
    cheap_task,
    None,
    tx1,
    None,
);
let cheap_result = rx1.recv().await;

// If result quality is insufficient, retry with better model
if !is_quality_sufficient(&cheap_result) {
    let better_task = TaskSpec::new("sub-better", task)
        .with_model("claude-4.5-sonnet");
    let (tx2, mut rx2) = mpsc::channel(1);
    spawn_subagent(
        provider,
        tools,
        workspace,
        better_task,
        None,
        tx2,
        None,
    );
    let better_result = rx2.recv().await;
}
```

### Parallel Model Comparison

```rust
// Run same task on multiple models, pick best result
let cheap_task = TaskSpec::new("sub-cheap", task.clone())
    .with_model("gpt-4o-mini");
let expensive_task = TaskSpec::new("sub-expensive", task)
    .with_model("claude-4.5-sonnet");

let (tx1, mut rx1) = mpsc::channel(1);
let (tx2, mut rx2) = mpsc::channel(1);
spawn_subagent(provider.clone(), tools.clone(), workspace.clone(), cheap_task, None, tx1, None);
spawn_subagent(provider, tools, workspace, expensive_task, None, tx2, None);

let (cheap_result, expensive_result) = tokio::join!(rx1.recv(), rx2.recv());

// Compare and select best result
```

---

## Configuration

### Model Aliases

Define model aliases in config:

```yaml
agents:
  models:
    fast:
      provider: openrouter
      model: openai/gpt-4o-mini
    
    coder:
      provider: openrouter
      model: anthropic/claude-4.5-sonnet
    
    reasoner:
      provider: deepseek
      model: deepseek-reasoner
```

Usage:

```rust
// Use alias via TaskSpec::with_model()
TaskSpec::new("sub-1", task).with_model("fast")
TaskSpec::new("sub-1", task).with_model("coder")
TaskSpec::new("sub-1", task).with_model("reasoner")
```

---

## Best Practices

1. **Match model to task complexity**: Don't use expensive models for simple tasks
2. **Consider latency**: Some models are slower than others
3. **Monitor costs**: Track token usage per model
4. **Have fallbacks**: If one model fails, try another
