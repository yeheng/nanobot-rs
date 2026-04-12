# Memory & History System

> AI's Brain Memory vs Conversation Notebook

---

## One-Sentence Understanding

- **Memory** = Long-term knowledge base (user preferences, learned notes)
- **History** = Short-term conversation log (chat records)

> Analogy: Memory is like a filing cabinet with organized folders. History is like a notebook you carry around for today's meetings.

---

## History System (Short-term Memory)

### What is History?

```mermaid
flowchart TB
    subgraph Today["Today's Conversation"]
        M1["You: Hello"]
        M2["AI: Hi there!"]
        M3["You: What's Rust?"]
        M4["AI: Rust is a systems language..."]
        M5["You: Thanks!"]
    end
    
    subgraph Storage["SQLite Storage"]
        DB[(session_events table)]
    end
    
    M1 --> DB
    M2 --> DB
    M3 --> DB
    M4 --> DB
    M5 --> DB
```

**History is the conversation transcript** - every message sent back and forth.

### Why Do We Need History?

| Without History | With History |
|-----------------|--------------|
| "What's my name?" → "I don't know" | "What's my name?" → "You're Alice!" |
| Each message is isolated | Context flows naturally |
| Can't reference earlier | "As you mentioned earlier..." |

### History Data Flow

```mermaid
sequenceDiagram
    participant User
    participant Session
    participant EventStore
    participant Kernel
    
    User->>Session: Send message
    Session->>EventStore: Save user message
    Session->>EventStore: Get recent history
    EventStore-->>Session: Last N messages
    Session->>Kernel: Pass context + history
    Kernel-->>Session: Return response
    Session->>EventStore: Save AI response
    Session-->>User: Show response
```

### What If History Gets Too Long?

```mermaid
flowchart TB
    History["Growing History"] --> Check{"Over token budget?"}
    Check -->|Yes| Evict["Evict old messages"]
    Check -->|No| Keep["Keep all"]
    Evict --> Summarize["Generate summary"]
    Summarize --> Store["Store summary in DB"]
    Store --> Continue["Continue with<br/>summary + recent"]
    Keep --> Continue
```

Like a notebook getting full - tear out old pages, keep a summary, continue writing.

---

## Memory System (Long-term Memory)

### What is Memory?

Unlike history (automatic recording), **memory is curated knowledge**:

```mermaid
flowchart TB
    subgraph Memory["📚 Long-term Memory"]
        direction TB
        P["👤 Profile<br/>Who you are"] 
        K["🧠 Knowledge<br/>What AI learned"]
        D["✅ Decisions<br/>Choices made"]
        A["📋 Active<br/>Current focus"]
        E["📖 Episodes<br/>Experiences"]
        R["🔗 Reference<br/>External info"]
    end
    
    subgraph History["📒 History"]
        H["Today's chat"]
    end
    
    History -.->|Extract important info| Memory
```

### Six Drawers of Memory

```mermaid
mindmap
  root((Memory))
    Profile
      ::icon(👤)
      Your name
      Preferences
      Communication style
    Active
      ::icon(📋)
      Current project
      TODO list
      Working on now
    Knowledge
      ::icon(🧠)
      Concepts learned
      Patterns found
      Best practices
    Decisions
      ::icon(✅)
      Choices made
      Rationale
      Alternatives considered
    Episodes
      ::icon(📖)
      Events experienced
      Problems solved
      Lessons learned
    Reference
      ::icon(🔗)
      Useful links
      Contact info
      Documentation
```

| Drawer | Purpose | Example |
|--------|---------|---------|
| Profile | Who you are | "User is a Rust developer, prefers concise answers" |
| Active | Current work | "Working on Project X, deadline next Friday" |
| Knowledge | Learned facts | "Rust uses ownership instead of GC" |
| Decisions | Past choices | "Chose SQLite over PostgreSQL for simplicity" |
| Episodes | Experiences | "Debugged a tricky async bug on 2024-01-15" |
| Reference | External info | "API docs: https://..." |

### Memory Temperature

```mermaid
flowchart LR
    subgraph Hot["🔥 Hot - Always Loaded"]
        H1[User name]
        H2[Current project]
    end
    
    subgraph Warm["🌡️ Warm - Often Loaded"]
        W1[Common preferences]
        W2[Relevant knowledge]
    end
    
    subgraph Cold["🧊 Cold - Search to Load"]
        C1[Old projects]
        C2[Background knowledge]
    end
    
    subgraph Archive["📦 Archived - Not Loaded"]
        A1[Very old info]
        A2[Superseded data]
    end
    
    Hot --> Warm --> Cold --> Archive
```

| Temperature | Load Strategy | Access Frequency |
|-------------|---------------|------------------|
| Hot | Always in context | Every conversation |
| Warm | If topic matches | Often |
| Cold | Search to find | Rarely |
| Archived | Not loaded unless asked | Almost never |

### Three-Phase Memory Loading

```mermaid
flowchart TB
    subgraph Phase1["Phase 1: Bootstrap (~700 tokens)"]
        P1[Load all Profile]
        P2[Load Active (hot only)]
    end
    
    subgraph Phase2["Phase 2: Scenario-aware (~1500 tokens)"]
        S1[Query hot items]
        S2[Query warm items<br/>with tag matching]
    end
    
    subgraph Phase3["Phase 3: On-demand (~1000 tokens)"]
        O1[Semantic search]
        O2[Load top results]
    end
    
    Phase1 --> Phase2 --> Phase3
    
    style Phase1 fill:#90EE90
    style Phase2 fill:#FFD700
    style Phase3 fill:#FFB6C1
```

1. **Bootstrap** (must have): Profile + current focus
2. **Scenario-aware** (likely relevant): Topic-matched memories
3. **On-demand** (search for): Specific query matching

**Hard limit**: 3200 tokens total

---

## Memory vs History: Complete Comparison

```mermaid
flowchart TB
    subgraph User["👤 User"]
        U["Uses both"]
    end
    
    subgraph Compare["Differences"]
        direction TB
        
        subgraph HistCol["📒 History"]
            H1["Automatic recording"]
            H2["Short-term (last 50 msg)"]
            H3["Raw conversation"]
            H4["Deleted after session"]
        end
        
        subgraph MemCol["📚 Memory"]
            M1["Curated storage"]
            M2["Long-term (forever)"]
            M3["Structured knowledge"]
            M4["Survives restart"]
        end
    end
    
    User --> HistCol
    User --> MemCol
```

| Aspect | History | Memory |
|--------|---------|--------|
| **What** | Conversation log | Curated knowledge |
| **When** | Automatic | Extracted/created manually |
| **How long** | Recent only | Forever |
| **Format** | Raw messages | Structured files |
| **Storage** | SQLite | Markdown files |
| **Persistence** | Session only | Permanent |
| **Growth** | Linear (every message) | Curated (important only) |

---

## Data Flow Panorama

```mermaid
flowchart TB
    subgraph Input["User Input"]
        Query["Query: 'What's my favorite color?'"]
    end
    
    subgraph Coord["Coordinator"]
        HC[HistoryCoordinator]
    end
    
    subgraph MemSys["Memory System"]
        MS[(MetadataStore)]
        ES[(EmbeddingStore)]
        FS[(FileMemoryStore)]
    end
    
    subgraph HistSys["History System"]
        SE[(EventStore)]
    end
    
    Query --> HC
    HC -->|Long-term memory| MS
    MS --> ES
    MS --> FS
    HC -->|Short-term history| SE
    
    HC -->|Combine results| Response
    
    subgraph Response["Final Context"]
        R1[Relevant memories]
        R2[Recent history]
    end
```

---

## Practical Scenarios

### Scenario 1: Remember User's Name

```
User: I'm Alice

[Session saves to History]
[AI extracts to Memory - Profile]

--- Next conversation ---

User: What's my name?
Session: [Loads Profile from Memory]
AI: You're Alice!
```

### Scenario 2: Continue Cross-Session Project

```
Session 1:
User: Working on Project X using Rust
[Saved to Active memory]

Session 2 (next day):
User: Any progress?
Session: [Loads Active memory]
AI: Last time we were working on Project X in Rust...
```

### Scenario 3: Smart Knowledge Recall

```
User: How do I handle errors in Rust?

Session: 
  1. [Semantic search Memory - Knowledge]
  2. [Found: "Rust error handling patterns"]
  3. [Load into context]

AI: Based on what we discussed before about Rust...
```

---

## FAQ

**Q: Will AI remember everything I say?**
A: No. History remembers recent conversations. Only important information is extracted to long-term Memory.

**Q: How do I make AI remember something?**
A: Explicitly ask: "Remember that I prefer dark mode" or edit `~/.gasket/memory/profile/preferences.md` directly.

**Q: Can I delete memories?**
A: Yes. Delete the corresponding `.md` file in `~/.gasket/memory/`.

**Q: Where is data stored?**
A: History in `~/.gasket/gasket.db` (SQLite), Memory in `~/.gasket/memory/` (Markdown files).
