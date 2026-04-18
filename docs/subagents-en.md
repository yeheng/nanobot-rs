# Subagent System

> AI Creating Clones

---

## One-Sentence Understanding

**Subagents are AI creating clones** - the main AI can spawn parallel workers to handle complex tasks.

> Analogy: Like a CEO delegating tasks to employees who work simultaneously and report back results.

---

## Why Do We Need Subagents?

```mermaid
flowchart TB
    subgraph Single["❌ Single Agent"]
        Task["Review 10 files"] --> Serial["File 1 → File 2 → ... → File 10"]
        Serial --> Slow["Takes 10 minutes"]
    end
    
    subgraph Parallel["✅ With Subagents"]
        Task2["Review 10 files"] --> Spawn["Spawn 10 subagents"]
        Spawn --> Work1["Agent 1: File 1"]
        Spawn --> Work2["Agent 2: File 2"]
        Spawn --> WorkN["Agent 10: File 10"]
        Work1 --> Collect["Collect Results"]
        Work2 --> Collect
        WorkN --> Collect
        Collect --> Fast["Takes 1 minute"]
    end
```

| Scenario | Single Agent | With Subagents |
|----------|--------------|----------------|
| Review 10 files | 10 minutes | 1 minute |
| Analyze multiple data sources | Sequential | Parallel |
| Complex workflow | Monolithic | Distributed |

---

## Core Concepts

```mermaid
flowchart TB
    subgraph Main["👔 Main Agent (CEO)"]
        Boss["Orchestrates tasks"]
    end
    
    subgraph Subagents["👷 Subagents (Employees)"]
        S1["Worker 1"]
        S2["Worker 2"]
        S3["Worker 3"]
    end
    
    subgraph Results["📊 Results"]
        R["Combined output"]
    end
    
    Boss -->|Delegates| S1
    Boss -->|Delegates| S2
    Boss -->|Delegates| S3
    
    S1 -->|Reports| R
    S2 -->|Reports| R
    S3 -->|Reports| R
    
    R --> Boss
```

**Key characteristics**:
- Main agent decides **what** to do
- Subagents decide **how** to do it
- All work **in parallel**
- Results **combined** at the end

---

## Use Cases

### 1. Code Review

```mermaid
flowchart LR
    PR["Pull Request<br/>10 files"] --> Spawn["Spawn 10 subagents"]
    
    Spawn --> A1["Review main.rs"]
    Spawn --> A2["Review lib.rs"]
    Spawn --> A3["Review utils.rs"]
    Spawn --> AN["Review test.rs"]
    
    A1 --> Summary
    A2 --> Summary
    A3 --> Summary
    AN --> Summary["Aggregate Report"]
```

### 2. Multi-Source Research

```
User: Research "Rust async runtime"

Main Agent spawns 3 subagents:
- Agent 1: Search official docs
- Agent 2: Search GitHub examples  
- Agent 3: Search blog posts

Results combined into comprehensive report
```

### 3. Data Processing

```
User: Analyze last month's logs

Main Agent spawns subagents by date:
- Agent 1: Week 1 logs
- Agent 2: Week 2 logs
- Agent 3: Week 3 logs
- Agent 4: Week 4 logs

Results aggregated for full analysis
```

---

## How It Works

```mermaid
sequenceDiagram
    participant Main as Main Agent
    participant Spawner as SpawnParallelTool
    participant Tracker as SubagentTracker
    participant S1 as Subagent 1
    participant S2 as Subagent 2
    participant User
    
    Main->>Spawner: spawn_parallel(tasks)
    Spawner->>Tracker: Create tracker
    
    loop For each task
        Spawner->>S1: spawn_subagent(task_spec, ...)
        Spawner->>S2: spawn_subagent(task_spec, ...)
    end
    
    par Result Collection
        Tracker->>Tracker: wait_for_all()
    and Event Streaming
        S1-->>Tracker: StreamEvent::Thinking
        S1-->>Tracker: StreamEvent::SubagentCompleted
        S2-->>Tracker: StreamEvent::Thinking
        S2-->>Tracker: StreamEvent::SubagentCompleted
    and WebSocket Forward
        Tracker-->>User: Forward events
    end
    
    Tracker->>Spawner: Vec<SubagentResult>
    Spawner->>Main: Aggregated output
```

---

## Three Execution Modes

```mermaid
flowchart TB
    subgraph Modes["Execution Modes"]
        direction TB
        
        subgraph FireForget["Fire-and-Forget"]
            FF1["Submit task"] --> FF2["Immediate return"]
            FF2 --> FF3["Background execution"]
        end
        
        subgraph SyncWait["Sync Wait"]
            SW1["Submit task"] --> SW2["Block and wait"]
            SW2 --> SW3["Return result"]
        end
        
        subgraph Parallel["Parallel (Recommended)"]
            P1["Submit multiple"] --> P2["Execute parallel"]
            P2 --> P3["Collect all results"]
        end
    end
```

| Mode | Use Case | Behavior |
|------|----------|----------|
| **Fire-and-Forget** | Background tasks | Submit and forget |
| **Sync Wait** | Sequential dependency | Wait for result |
| **Parallel** | Batch processing | Multiple simultaneous |

### Mode 1: Fire-and-Forget

```rust
// Spawn and continue immediately
let (result_tx, mut result_rx) = mpsc::channel(1);
spawn_subagent(
    provider,
    tools,
    workspace,
    TaskSpec::new("sub-1", "Summarize this article"),
    None,
    result_tx,
    None,
    CancellationToken::new(),
);
// Returns immediately, runs in background
```

### Mode 2: Sync Wait

```rust
// Block until complete
let (result_tx, mut result_rx) = mpsc::channel(1);
spawn_subagent(
    provider,
    tools,
    workspace,
    TaskSpec::new("sub-1", "Analyze this code"),
    None,
    result_tx,
    None,
    CancellationToken::new(),
);
let result = result_rx.recv().await;
// Use result in main agent
```

### Mode 3: Parallel (Recommended)

```rust
// Spawn multiple subagents with a tracker
let mut tracker = SubagentTracker::new();
for (id, task) in tasks {
    let task = TaskSpec::new(&id, task)
        .with_system_prompt("Code reviewer".to_string());
    spawn_subagent(
        provider.clone(),
        tools.clone(),
        workspace.clone(),
        task,
        Some(tracker.event_sender()),
        tracker.result_sender(),
        Some(token_tracker.clone()),
        tracker.cancellation_token(),
    );
}
let results = tracker.wait_for_all(tasks.len()).await?;
```

---

## Subagent Events

Real-time progress tracking:

```mermaid
sequenceDiagram
    participant Sub as Subagent
    participant Tracker
    participant User
    
    Sub->>Tracker: StreamEvent::Thinking
    Tracker-->>User: "Thinking..."
    
    Sub->>Tracker: StreamEvent::ToolStart
    Tracker-->>User: "Using tool: read_file"
    
    Sub->>Tracker: StreamEvent::ToolEnd
    Tracker-->>User: "Tool completed"
    
    Sub->>Tracker: StreamEvent::Content
    Tracker-->>User: "Partial result..."
    
    Sub->>Tracker: StreamEvent::SubagentCompleted
    Tracker-->>User: "Task done!"
```

### Event Types

```rust
// Unified StreamEvent is used for both main agent and subagent.n// Subagent events have agent_id set to Some(subagent_uuid).n// See docs/data-structures-en.md for the full StreamEvent definition.n
```

---

## State Management

```mermaid
flowchart TB
    subgraph Persistent["💾 Persistent (Main Agent)"]
        P1[Remembers context]
        P2[Saves to database]
        P3[Long-term memory]
    end
    
    subgraph Stateless["🔄 Stateless (Subagent)"]
        S1[Fresh context]
        S2[No persistence]
        S3[Task-focused]
    end
    
    User --> Persistent
    Persistent -->|Spawns| Stateless
```

Subagents are **stateless** by design:
- No access to main agent's conversation history
- No long-term memory
- Focused only on the assigned task

This ensures:
- ✅ Clean separation of concerns
- ✅ Isolated failure domains
- ✅ Easier debugging
- ✅ Resource efficiency

---

## Model Selection

Subagents can use different models than the main agent:

```rust
// Fast/cheap model for simple tasks
let task = TaskSpec::new("sub-1", prompt)
    .with_model("gpt-4o-mini")
    .with_system_prompt(system_prompt);
spawn_subagent(provider, tools, workspace, task, None, result_tx, None);

// Powerful model for complex tasks
let task = TaskSpec::new("sub-2", prompt)
    .with_model("claude-4.5-sonnet")
    .with_system_prompt(system_prompt);
spawn_subagent(provider, tools, workspace, task, None, result_tx, None);
```

| Task Type | Recommended Model | Why |
|-----------|-------------------|-----|
| Simple extraction | gpt-4o-mini | Fast, cheap |
| Code review | claude-4.5-sonnet | Better at code |
| Creative writing | claude-4.5-sonnet | More creative |
| Data analysis | deepseek-chat | Good at structured output |

---

## Architecture

### Components

```mermaid
flowchart TB
    subgraph Management["Management Layer"]
        ST[SubagentTracker]
        TS[TaskSpec]
    end
    
    subgraph Execution["Execution Layer"]
        R1[Runner 1]
        R2[Runner 2]
        RN[Runner N]
    end
    
    subgraph Communication["Communication"]
        CH[mpsc channels]
        WS[WebSocket]
    end
    
    TS -->|spawns| R1
    TS -->|spawns| R2
    TS -->|spawns| RN
    
    R1 -->|events| CH
    R2 -->|events| CH
    RN -->|events| CH
    
    CH --> ST
    ST -->|forward| WS
```

### spawn_subagent

Core pure-function API for spawning a subagent:

```rust
pub fn spawn_subagent(
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    workspace: PathBuf,
    task: TaskSpec,
    event_tx: Option<mpsc::Sender<StreamEvent>>,
    result_tx: mpsc::Sender<SubagentResult>,
    token_tracker: Option<Arc<TokenTracker>>,
    cancellation_token: CancellationToken,
) -> JoinHandle<()>
```

### SubagentTracker

Monitors all running subagents:

```rust
pub struct SubagentTracker {
    result_tx: mpsc::Sender<SubagentResult>,
    result_rx: Option<mpsc::Receiver<SubagentResult>>,
    event_tx: mpsc::Sender<StreamEvent>,
    event_rx: Option<mpsc::Receiver<StreamEvent>>,
    cancellation_token: CancellationToken,
}
```

---

## Best Practices

### 1. Task Granularity

```
✅ Good: "Review this specific function"
❌ Bad: "Review entire codebase"

✅ Good: "Extract dates from this log"
❌ Bad: "Analyze all logs"
```

### 2. Error Handling

```rust
// Always handle subagent failures
match result_rx.recv().await {
    Some(result) => process(result),
    None => {
        log!("Subagent failed or channel closed");
        // Fallback or retry
    }
}
```

### 3. Resource Limits

```yaml
# config.yaml
# Note: subagent_limits is not currently implemented.
# Subagent timeout defaults to 600 seconds.
agents:
  defaults:
    max_tokens: 2000        # Token limit per request
```

### 4. Result Aggregation

```rust
// Combine results intelligently
let results = tracker.wait_for_all().await;
let combined = results
    .into_iter()
    .map(|r| r.content)
    .collect::<Vec<_>>()
    .join("\n---\n");
```

---

## Complete Example

```rust
// Main agent decides to review 5 files
let files = vec!["main.rs", "lib.rs", "utils.rs", "tests.rs", "config.rs"];

// Create tracker for progress monitoring
let tracker = SubagentTracker::new();

// Spawn subagents for each file
for (i, file) in files.iter().enumerate() {
    let task_id = format!("review-{}", i);
    let prompt = format!("Review {} for code quality", file);
    
    manager
        .task(&task_id, &prompt)
        .with_system_prompt("You are a code reviewer".into())
        .with_streaming(tracker.event_sender())
        .spawn(tracker.result_sender())
        .await?;
}

// Wait for all to complete
let results = tracker.wait_for_all().await;

// Aggregate into final report
let report = generate_report(results);
```

---

## Related Modules

- **Kernel**: Executes subagent tasks
- **Session**: Provides isolated context
- **Tools**: `spawn_parallel` tool triggers subagents
