# Session Module

> The "Butler" of Conversations

---

## One-Sentence Understanding

**Session is your personal butler** - it manages the complete lifecycle of a conversation, from greeting to farewell.

> Analogy: Like a hotel concierge who knows your preferences, arranges everything, and ensures your stay is perfect.

---

## Why Do We Need Session?

```mermaid
flowchart TB
    subgraph Without["❌ Without Session"]
        U1[User] --> K1[Kernel]
        K1 -->|"No memory"| A1[Each message is isolated]
    end
    
    subgraph With["✅ With Session"]
        U2[User] --> S2[Session]
        S2 -->|Load history| S2
        S2 -->|Inject memory| S2
        S2 --> K2[Kernel]
        K2 --> S2
        S2 -->|Save history| S2
        S2 --> U2
    end
```

| Feature | Without Session | With Session |
|---------|-----------------|--------------|
| Remember context | ❌ No | ✅ Yes |
| Long-term memory | ❌ No | ✅ Yes |
| History compression | ❌ No | ✅ Yes |
| Multi-turn dialogue | ❌ Hard | ✅ Easy |

---

## Core Responsibilities

```mermaid
flowchart TB
    subgraph Session["📋 Session Butler"]
        direction TB
        Greet["1. Greeting<br/>Load user profile"]
        Prep["2. Preparation<br/>Assemble context"]
        Delegate["3. Delegation<br/>Call Kernel"]
        Record["4. Record<br/>Save conversation"]
        Farewell["5. Farewell<br/>Compress if needed"]
    end
    
    User --> Greet
    Greet --> Prep
    Prep --> Delegate
    Delegate --> Record
    Record --> Farewell
    Farewell --> User
```

### 1. Greeting Phase

When you arrive at the hotel (start chatting):

```mermaid
sequenceDiagram
    participant User
    participant Session
    participant Memory
    
    User->>Session: Start conversation
    Session->>Memory: Load PROFILE.md
    Session->>Memory: Load MEMORY.md
    Session->>Memory: Load relevant knowledge
    Memory-->>Session: User preferences
    Session-->>User: "Hello! I remember you like..."
```

**What gets loaded:**
- Profile (who you are)
- Active memories (current focus)
- Relevant knowledge (based on query)

### 2. Preparation Phase

Before the "brain" (Kernel) starts working:

```mermaid
flowchart LR
    subgraph Assembly["Context Assembly"]
        direction TB
        Sys["[system] PROFILE.md + SOUL.md + AGENTS.md + BOOTSTRAP.md + skills"] --> Combine
        Mem["[user] Dynamic memory (relevant memories + summary)"] --> Combine
        History["[user/assistant] Recent History"] --> Combine
        Current["[user] Current Input"] --> Combine
        Combine["Combine & Truncate"] --> Final["Final Context"]
    end
```

**Assembly order** (like making a sandwich):
1. Bottom: `[system]` - Static workspace markdown (PROFILE.md, SOUL.md, etc.)
2. Layer: `[user]` - Dynamic memory content (relevant memories, summary)
3. Layer: `[user/assistant]` - Conversation history
4. Top: `[user]` - Current user message

> **Prompt Cache Protection**: Dynamic memory is injected as a User Message rather than appended to the system prompt. This preserves the Prompt Cache for the static system content, reducing token costs and latency.

### 3. Delegation Phase

Hand over to Kernel (the brain):

```
Session: "Kernel, here's everything you need:
         - System prompt
         - User's background
         - Conversation history
         - Available tools
         
         Please process this and give me an answer."
```

### 4. Recording Phase

Save the conversation:

```mermaid
flowchart LR
    Response["AI Response"] --> Save["Save to SQLite"]
    Save --> Index["Update indexes"]
    Index --> Embed["Generate embeddings"]
```

### 5. Farewell Phase

When conversation gets too long, compress it:

```mermaid
flowchart TB
    Check{"History too long?"} -->|Yes| Summarize["LLM Summarization"]
    Check -->|No| Done["Done"]
    Summarize --> Store["Store summary + watermark"]
    Store --> Clean["Clean old messages"]
    Clean --> Done
```

---

## Two Types of Sessions

```mermaid
flowchart TB
    subgraph Persistent["💾 Persistent Session"]
        P1[Main Agent]
        P2[Saved to SQLite]
        P3[Has long-term memory]
        P4[Survives restart]
    end
    
    subgraph Stateless["🔄 Stateless Session"]
        S1[Subagent]
        S2[In-memory only]
        S3[No persistence]
        S4[Temporary task]
    end
    
    User --> P1
    P1 -->|Spawns| S1
```

| Feature | Persistent | Stateless |
|---------|-----------|-----------|
| Use case | Main conversation | Background tasks |
| Storage | SQLite | Memory only |
| History | Kept indefinitely | Lost after task |
| Memory | Full access | No access |

---

## History Management

### Three-Phase History Loading

```mermaid
flowchart TB
    subgraph Phase1["Phase 1: Recent Messages"]
        R1[Last N messages]
        R2[Always keep recent]
    end
    
    subgraph Phase2["Phase 2: Summary"]
        S1[Older conversations]
        S2[Compressed by LLM]
    end
    
    subgraph Phase3["Phase 3: Relevant"]
        E1[Semantic search]
        E2[Similar past messages]
    end
    
    Phase1 --> Phase2 --> Phase3
```

### Token Budget

Like a suitcase with limited space:

```
Memory Token Budgets (defaults):

Bootstrap:        1500 tokens ████████░░
Scenario:         1500 tokens ████████░░
On-demand:        1000 tokens █████░░░░░
Total Cap:        4000 tokens ██████████
────────────────────────────────────────
```

---

## Context Compression

When the suitcase is full, compress old clothes:

```mermaid
sequenceDiagram
    participant Session
    participant Compactor
    participant LLM
    participant Storage
    
    Session->>Compactor: Check token usage
    Compactor->>Compactor: Over budget?
    Compactor->>LLM: Summarize old messages
    LLM-->>Compactor: Summary text
    Compactor->>Storage: Save summary
    Compactor->>Storage: Delete old messages
    Compactor-->>Session: Done
```

---

## Hook Integration

Session integrates with hooks at key points:

```mermaid
flowchart TB
    User["User Input"] --> Hook1["BeforeRequest Hook"]
    Hook1 --> Greet["Load Session"]
    Greet --> Hook2["AfterHistory Hook"]
    Hook2 --> Prep["Prepare Context"]
    Prep --> Hook3["BeforeLLM Hook"]
    Hook3 --> Kernel["Kernel Execution"]
    Kernel --> Hook4["AfterToolCall Hook"]
    Hook4 --> Save["Save Response"]
    Save --> Hook5["AfterResponse Hook"]
    Hook5 --> User
```

---

## Tool Approval Flow

When AI calls a tool that requires approval, Session coordinates the approval process:

```mermaid
sequenceDiagram
    participant User
    participant Session
    participant Kernel
    participant Registry as ToolRegistry
    participant CB as ApprovalCallback
    
    User->>Session: Create a file for me
    Session->>Kernel: Process message
    Kernel->>Registry: Execute write_file
    Registry->>Registry: requires_approval = true
    Registry->>CB: request_approval
    
    alt WebSocket Mode
        CB->>User: Show confirmation dialog
        User-->>CB: Approve / Deny / Remember
    else CLI Mode
        CB->>CB: No callback, execute directly
    end
    
    CB-->>Registry: Approval result
    alt Approved
        Registry->>Registry: Execute write_file
        Registry-->>Kernel: Result
    else Denied
        Registry-->>Kernel: PermissionDenied
    end
    Kernel-->>Session: Response
    Session-->>User: File created / Operation denied
```

**WebSocket Mode Features:**
- Frontend shows a confirmation dialog with tool name, description, and arguments
- Users can check "Remember this decision" to auto-approve/deny future calls of the same tool in the same session
- Timeout without response is treated as denial

---

## Key Data Structures

### AgentSession

The butler's toolkit:

```rust
struct AgentSession {
    runtime_ctx: RuntimeContext,         // Execution dependencies
    event_store: Arc<EventStore>,        // Event persistence (non-optional)
    session_store: Arc<SessionStore>,    // Session storage (non-optional)
    config: AgentConfig,                 // Behavior settings
    system_prompt: String,               // AI personality
    hooks: Arc<HookRegistry>,            // Extension points
    compactor: Option<Arc<ContextCompactor>>, // Compression
    pricing: Option<ModelPricing>,       // Cost calculation
    finalizer: ResponseFinalizer,        // Response post-processing
    pending_done: TaskTracker,           // Graceful shutdown tracker
}
```

---

## Session Lifecycle

```mermaid
stateDiagram-v2
    [*] --> Created: New session
    Created --> Active: First message
    Active --> Active: Continue chatting
    Active --> Compressed: History too long
    Compressed --> Active: Keep chatting
    Active --> Idle: No activity
    Idle --> Active: New message
    Idle --> Destroyed: Timeout
    Destroyed --> [*]
```

---

## Related Modules

- **Kernel**: The "brain" that Session delegates to
- **Memory**: Long-term storage that Session manages
- **Hooks**: Extension points during Session lifecycle
- **Storage**: SQLite backend for persistence
