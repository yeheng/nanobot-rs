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
let result = manager
    .submit_and_wait_with_model(
        "Summarize this text".to_string(),
        "You are a summarizer".to_string(),
        channel,
        chat_id,
        "gpt-4o-mini"  // Cheaper model for simple task
    )
    .await?;
```

### With Streaming

```rust
// Spawn with streaming and specific model
let result = manager
    .submit_and_wait_with_model_streaming(
        "Analyze this code".to_string(),
        "You are a code reviewer".to_string(),
        channel,
        chat_id,
        "claude-4.5-sonnet",  // Better model for code
        event_tx
    )
    .await?;
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
let result = manager
    .submit_and_wait_with_model(
        task.clone(),
        prompt.clone(),
        channel,
        chat_id,
        "gpt-4o-mini"  // Try cheap model first
    )
    .await;

// If result quality is insufficient, retry with better model
if !is_quality_sufficient(&result) {
    let result = manager
        .submit_and_wait_with_model(
            task,
            prompt,
            channel,
            chat_id,
            "claude-4.5-sonnet"  // Use better model
        )
        .await?;
}
```

### Parallel Model Comparison

```rust
// Run same task on multiple models, pick best result
let cheap = manager.spawn_with_model(task.clone(), "gpt-4o-mini");
let expensive = manager.spawn_with_model(task.clone(), "claude-4.5-sonnet");

let (cheap_result, expensive_result) = tokio::join!(cheap, expensive);

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
// Use alias
.spawn_with_model(task, "fast")
.spawn_with_model(task, "coder")
.spawn_with_model(task, "reasoner")
```

---

## Best Practices

1. **Match model to task complexity**: Don't use expensive models for simple tasks
2. **Consider latency**: Some models are slower than others
3. **Monitor costs**: Track token usage per model
4. **Have fallbacks**: If one model fails, try another
