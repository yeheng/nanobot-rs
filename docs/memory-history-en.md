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

## Memory System (Long-term Memory) — Wiki Knowledge Base

### What is Memory?

Unlike history (automatic recording), **memory is curated knowledge** stored in the Wiki system:

```mermaid
flowchart TB
    subgraph Memory["📚 Wiki Knowledge Base"]
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

### Six Categories of Wiki Pages

```mermaid
mindmap
  root((Wiki Pages))
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

| Category | Purpose | Example |
|----------|---------|---------|
| Profile | Who you are | "User is a Rust developer, prefers concise answers" |
| Active | Current work | "Working on Project X, deadline next Friday" |
| Knowledge | Learned facts | "Rust uses ownership instead of GC" |
| Decisions | Past choices | "Chose SQLite over PostgreSQL for simplicity" |
| Episodes | Experiences | "Debugged a tricky async bug on 2024-01-15" |
| Reference | External info | "API docs: https://..." |

### Memory Types: Note vs Skill

Wiki pages have a `memory_type` field that controls how they are loaded and prioritized:

| Type | Purpose | Example |
|------|---------|---------|
| `note` | Facts, observations, learned knowledge | "User prefers dark mode" |
| `skill` | Procedures, workflows, reusable patterns | "How to deploy a Rust service" |

**Skill pages are prioritized during loading** - they are loaded before `note` pages within each phase, ensuring procedural knowledge is always available.

---

### Memory Temperature (Frequency)

Page access frequency is tracked by the `Frequency` enum (`gasket_storage::wiki::types::Frequency`) with four levels:

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

| Frequency | Load Strategy | Access Frequency | Decay Threshold |
|-----------|---------------|------------------|-----------------|
| Hot | Always in context | Every conversation | → Warm after 7 days |
| Warm | If topic matches | Often | → Cold after 30 days |
| Cold | Search to find | Rarely | → Archived after 90 days |
| Archived | Not loaded unless asked | Almost never | — |

Frequency decay is managed by `FrequencyManager` (`engine/src/wiki/lifecycle.rs`) and executed via `WikiDecayTool` (tool name: `wiki_decay`) as a system cron job.

The following paths are **exempt from decay** and always stay Hot: `profile/*`, `entities/people/*`, `sops/*`, `sources/*`, and any path containing `/decisions/`.

### Three-Phase Memory Loading

Token budgets are defined by `TokenBudget` (`gasket_storage::wiki::TokenBudget`) with the same defaults:

```mermaid
flowchart TB
    subgraph Phase1["Phase 1: Bootstrap (~1500 tokens)"]
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
2. **Scenario-aware** (likely relevant): Topic-matched wiki pages
3. **On-demand** (search for): Specific query matching

**Hard limit**: ~4000 tokens total. Results are injected as a **User Message** (not appended to the system prompt) to preserve Prompt Cache.

---

## Memory (Wiki) vs History: Complete Comparison

```mermaid
flowchart TB
    subgraph User["👤 User"]
        U["Uses both"]
    end
    
    subgraph Compare["Differences"]
        direction TB
        
        subgraph HistCol["📒 History"]
            H1["Automatic recording"]
            H2["Permanent in SQLite"]
            H3["Raw conversation"]
            H4["Survives restart"]
        end
        
        subgraph MemCol["📚 Memory (Wiki)"]
            M1["Curated storage"]
            M2["Long-term (forever)"]
            M3["Structured wiki pages"]
            M4["Survives restart"]
        end
    end
    
    User --> HistCol
    User --> MemCol
```

| Aspect | History | Memory (Wiki) |
|--------|---------|--------|
| **What** | Conversation log | Curated knowledge |
| **When** | Automatic | Extracted/created manually |
| **How long** | Forever (compacted) | Forever |
| **Format** | Raw messages | Structured wiki pages |
| **Storage** | SQLite (`session_events`) | SQLite (wiki tables) |
| **Persistence** | Permanent | Permanent |
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
    
    subgraph MemSys["Wiki Knowledge System"]
        WS[(Wiki Store<br/>SQLite)]
        TI[(Search Index<br/>Tantivy)]
    end
    
    subgraph HistSys["History System"]
        SE[(EventStore)]
    end
    
    Query --> HC
    HC -->|Long-term memory| WS
    WS --> TI
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
[AI extracts to Wiki - Profile page]

--- Next conversation ---

User: What's my name?
Session: [Loads Profile from Wiki]
AI: You're Alice!
```

### Scenario 2: Continue Cross-Session Project

```
Session 1:
User: Working on Project X using Rust
[Saved to Active wiki page]

Session 2 (next day):
User: Any progress?
Session: [Loads Active wiki page]
AI: Last time we were working on Project X in Rust...
```

### Scenario 3: Smart Knowledge Recall

```
User: How do I handle errors in Rust?

Session:
  1. [Semantic search Wiki - Knowledge]
  2. [Found: "Rust error handling patterns"]
  3. [Load into context]

AI: Based on what we discussed before about Rust...
```

---

## FAQ

**Q: Will AI remember everything I say?**
A: No. History remembers recent conversations. Only important information is extracted to long-term Memory.

**Q: How do I make AI remember something?**
A: Explicitly ask: "Remember that I prefer dark mode" or use the `memorize` tool / Wiki CLI to manage pages.

**Q: Can I delete memories?**
A: Yes. Use the `wiki_write` tool to remove pages, or manage them via the Wiki CLI (`gasket wiki`).

**Q: Where is data stored?**
A: History in `~/.gasket/gasket.db` (SQLite), Wiki knowledge in `~/.gasket/wiki/` (SQLite with optional Tantivy search index).
