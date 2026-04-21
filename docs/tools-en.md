# Tools System

> The "Hands and Feet" of AI

---

## One-Sentence Understanding

**Tools are AI's hands and feet** - they allow AI to interact with the outside world beyond just talking.

> Analogy: Like a person with hands can write, cook, and build; AI with tools can search, code, and execute commands.

---

## Why Do We Need Tools?

```mermaid
flowchart TB
    subgraph NoTools["❌ AI Without Tools"]
        Q1["User: What's the weather?"]
        A1["AI: Sorry, I don't know..."]
        
        Q2["User: Read this file"]
        A2["AI: I can't access files..."]
    end
    
    subgraph WithTools["✅ AI With Tools"]
        Q3["User: What's the weather?"]
        T1["Search weather API"] --> A3["AI: It's sunny, 25°C"]
        
        Q4["User: Read this file"]
        T2["Read file tool"] --> A4["AI: File content is..."]
    end
```

Without tools, AI is just a "brain in a jar" - smart but powerless.

---

## What Can Tools Do?

```mermaid
mindmap
  root((Tools))
    Filesystem
      ::icon(📁)
      Read files
      Write files
      List directories
    Web
      ::icon(🌐)
      Search web
      Fetch URLs
    System
      ::icon(⚡)
      Run commands
      Spawn subagents
    Communication
      ::icon(💬)
      Send messages
    Memory
      ::icon(🧠)
      Search memories
      Create memories
    Wiki
      ::icon(📚)
      Search wiki
      Read/write pages
      Decay and refresh
    Schedule
      ::icon(⏰)
      Create cron jobs
```

---

## Tool Categories

### 1. Filesystem Tools

```mermaid
sequenceDiagram
    participant User
    participant AI
    participant Tool as read_file Tool
    participant FS as File System
    
    User->>AI: Read main.rs
    AI->>Tool: Call read_file
    Tool->>FS: Read file
    FS-->>Tool: Return content
    Tool-->>AI: File content
    AI-->>User: Explain the code
```

| Tool | Purpose | Example |
|------|---------|---------|
| `read_file` | Read file content | "Read config.yaml" |
| `write_file` | Create new file | "Create hello.py" |
| `edit_file` | Modify existing file | "Add function to main.rs" |
| `list_dir` | List directory | "Show files in src/" |

### 2. Web Tools

```mermaid
flowchart LR
    User["User: Search Rust tutorial"] --> AI
    AI --> Search["web_search Tool"]
    Search --> API["Search API<br/>(Brave/Tavily)"]
    API --> Results["Search Results"]
    Results --> AI --> Answer["Curated Answer"]
```

| Tool | Purpose | Example |
|------|---------|---------|
| `web_search` | Search the web | "Find latest Rust version" |
| `web_fetch` | Fetch specific URL | "Read this article" |

### 3. System Tools

```mermaid
flowchart TB
    subgraph Safe["✅ Safe Commands"]
        S1["git status"]
        S2["cargo build"]
        S3["ls -la"]
    end
    
    subgraph Dangerous["❌ Blocked Commands"]
        D1["rm -rf /"]
        D2["curl evil.com"]
    end
    
    subgraph Policy["Command Policy"]
        P1["allow_list<br/>Only permitted"]
        P2["deny_list<br/>Block dangerous"]
        P3["allow_all<br/>Everything (risky!)"]
    end
```

| Tool | Purpose | Safety |
|------|---------|--------|
| `exec` | Run shell commands | Configurable policy |
| `spawn` | Create subagent | Isolated execution |

### 4. Communication Tools

```mermaid
sequenceDiagram
    participant Main as Main Agent
    participant Tool as send_message Tool
    participant User
    
    Main->>Tool: Send "Task done!"
    Tool->>User: Telegram message
    User-->>Tool: Reply
    Tool-->>Main: Response
```

| Tool | Purpose | Example |
|------|---------|---------|
| `send_message` (`MessageTool`) | Send to channel | "Notify user on Telegram" |

### 5. Memory Tools

```mermaid
flowchart LR
    Query["User query"] --> Search["memory_search"]
    Search --> DB[(SQLite + Vectors)]
    DB --> Results["Relevant memories"]
    Results --> AI["AI with context"]
```

| Tool | Purpose | Example |
|------|---------|---------|
| `memory_search` | Search long-term memory | "What did I learn about DB?" |
| `memorize` | Create new memory | "Remember my API key is..." |

The `memorize` tool accepts a `memory_type` parameter:
- `"note"` (default) - Facts, observations, learned knowledge
- `"skill"` - Procedures, workflows, reusable patterns (prioritized during loading)

```mermaid
sequenceDiagram
    participant AI
    participant Tool as memorize Tool
    participant Store as MemoryStore

    AI->>Tool: memorize(content, memory_type)
    Note over Tool: Determine type:
    Note over Tool: "how to..." / "steps..." → skill
    Note over Tool: "user likes..." / "learned..." → note
    Tool->>Store: Save with type tag
    Store-->>Tool: OK
    Tool-->>AI: Memory created
```

### 5.1 Wiki Tools

Wiki tools provide structured knowledge management powered by Tantivy BM25 full-text search and SQLite storage:

```mermaid
sequenceDiagram
    participant U as User
    participant AI as AI
    participant W as Wiki Tool
    participant Store as SQLite + Tantivy

    U->>AI: Search for Rust ownership info

    AI->>W: wiki_search("Rust ownership")
    W->>Store: Tantivy BM25 search
    Store-->>W: Matching wiki pages
    W-->>AI: Search results list

    AI->>W: wiki_read("rust/ownership")
    W->>Store: Read page details
    Store-->>W: Full Markdown content
    W-->>AI: Page content

    AI-->>U: Based on the wiki, Rust ownership means...

    U->>AI: Save this summary to the knowledge base
    AI->>W: wiki_write("rust/summary", ...)
    W->>Store: Write page + update index
    Store-->>W: Saved
    W-->>AI: Page created
```

| Tool | Purpose | Parameters |
|------|---------|------------|
| `wiki_search` (`WikiSearchTool`) | Search wiki pages using Tantivy BM25 | `query` (required), `limit` (optional, default 10) |
| `wiki_write` (`WikiWriteTool`) | Write/update a wiki page | `path`, `title`, `content` (required), `page_type` (optional, default `"topic"`), `tags` (optional array) |
| `wiki_read` (`WikiReadTool`) | Read a wiki page by path | `path` (required). Returns full Markdown content with metadata. |
| `wiki_decay` (`WikiDecayTool`) | Run automated frequency decay on wiki pages | No parameters required. Zero LLM cost. Returns summary of scanned/decayed/errored pages. |
| `wiki_refresh` (`WikiRefreshTool`) | Sync on-disk Markdown files into SQLite and Tantivy | `action`: `"sync"` (incremental), `"reindex"` (full rebuild), `"stats"` (statistics) |

### 6. Schedule Tools

| Tool | Purpose | Example |
|------|---------|---------|
| `cron` | Create scheduled task | "Remind me daily at 9am" |
| `script` (`PluginTool`) | External script with YAML manifest | Custom business logic |

---

## How AI Uses Tools

```mermaid
sequenceDiagram
    participant User
    participant AI
    participant Kernel
    participant Tool
    
    User->>AI: "What's the weather in Beijing?"
    
    AI->>Kernel: Need to use tool?
    Kernel->>AI: Yes, call web_search
    
    AI->>Tool: web_search("Beijing weather")
    Tool->>Tool: Call weather API
    Tool-->>AI: {"temp": 25, "condition": "Sunny"}
    
    AI->>Kernel: Now I have info
    Kernel-->>AI: Generate response
    AI-->>User: "It's sunny and 25°C in Beijing"
```

### Decision Flow

```mermaid
flowchart TB
    Input["User Input"] --> Think["AI Thinking"]

    Think --> Decision{"Need tool?"}

    Decision -->|Yes| Which{"Which tool?"}
    Decision -->|No| Direct["Direct Answer"]

    Which -->|File| FileTool["Read/Write File"]
    Which -->|Info| WebTool["Web Search"]
    Which -->|Command| ExecTool["Execute Command"]
    Which -->|Memory| MemTool["Search Memory"]
    Which -->|Knowledge| WikiTool["Search Wiki"]

    FileTool --> Result["Tool Result"]
    WebTool --> Result
    ExecTool --> Result
    MemTool --> Result
    WikiTool --> Result

    Result --> ThinkAgain["Think Again"]
    ThinkAgain --> Decision

    Direct --> Output["Final Response"]
```

---

## Tool Execution

### Parallel Execution

When AI needs multiple tools, they run in parallel:

```mermaid
flowchart TB
    AI["AI Decision"] -->|"Need 3 files"| Exec["ToolExecutor"]
    
    subgraph Parallel["Parallel Execution"]
        direction LR
        T1["read_file A"]
        T2["read_file B"]
        T3["read_file C"]
    end
    
    Exec --> T1
    Exec --> T2
    Exec --> T3
    
    T1 --> Results["Combined Results"]
    T2 --> Results
    T3 --> Results
    
    Results --> AI
```

Example: "Compare file1.rs, file2.rs, and file3.rs"
- All three files are read simultaneously
- Results combined and sent back to AI

### Tool Context

Tools receive context about the current session:

```rust
struct ToolContext {
    session_key: SessionKey,     // Who is asking
    outbound_tx: Sender<OutboundMessage>, // Real-time message channel
    spawner: Arc<dyn SubagentSpawner>,    // Subagent spawner
    token_tracker: Arc<TokenTracker>,     // Token budget tracker
}
```

This allows tools to:
- Know which user/session
- Access allowed directories
- Respect configuration limits

---

## Tool Registry

All available tools are registered in a central registry:

```mermaid
flowchart TB
    subgraph Registry["ToolRegistry"]
        T1["read_file"]
        T2["write_file"]
        T3["web_search"]
        T4["exec"]
        T5["spawn"]
        T6["memory_search"]
        T7["wiki_search"]
        T8["wiki_write"]
        T9["wiki_read"]
        T10["wiki_decay"]
        T11["wiki_refresh"]
        T12["history_query"]
        T13["script"]
        TN["...more"]
    end

    Kernel -->|Query| Registry
    Registry -->|Return list| Kernel
    Kernel -->|Select| Selected["Appropriate tools"]
```

### Tool Execution Signature

All tools implement the `Tool` trait. The `ctx` parameter is **required**:

```rust
async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult;
```

### Tool Definition Format

Each tool defines:

```json
{
  "name": "read_file",
  "description": "Read content of a file",
  "parameters": {
    "type": "object",
    "properties": {
      "path": {
        "type": "string",
        "description": "Path to the file"
      }
    },
    "required": ["path"]
  }
}
```

This is the **JSON Schema** that tells AI how to use the tool.

---

## Safety Design

### Command Policy

```yaml
tools:
  exec:
    policy:
      allowlist: ["git", "cargo", "ls", "cat"]
      denylist: ["rm", "sudo"]
```

| Policy | Description | Risk Level |
|--------|-------------|------------|
| `allow_list` | Only allow specific commands | 🟢 Low |
| `deny_list` | Block dangerous commands | 🟡 Medium |
| `allow_all` | Allow everything | 🔴 High |

### Path Restrictions

```yaml
tools:
  filesystem:
    allowed_paths:
      - "~/projects/"
      - "~/.gasket/"
    blocked_paths:
      - "~/.ssh/"
      - "/etc/"
```

---

## MCP: External Tools

Model Context Protocol allows connecting external tool servers:

```mermaid
flowchart LR
    Gasket["Gasket"] <-->|JSON-RPC| MCP["MCP Server"]
    MCP --> DB[(Database)]
    MCP --> API["External API"]
    MCP --> Custom["Custom Tools"]
```

Example MCP servers:
- Database query tools
- GitHub integration
- Custom business tools

---

## Related Modules

- **Kernel**: Decides when to use tools
- **Sandbox**: Isolates tool execution
- **Session**: Provides tool context
