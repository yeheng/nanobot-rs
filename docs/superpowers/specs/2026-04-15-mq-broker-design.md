# In-Process Message Broker (gasket-mq) Design Spec

**Date:** 2026-04-15
**Status:** Approved (Rev.2 — post spec-review fixes)
**Scope:** Replace existing Bus + Actor pipeline with a unified Topic-based Message Broker

---

## 1. Problem Statement

The current `MessageBus` (`engine/src/bus/`) is a fixed two-channel Inbound/Outbound pipeline with a three-actor model (Router → Session → Outbound). While functional, it has concrete limitations:

1. **Silent error swallowing** — `publish_inbound/outbound` log errors but never return `Result`, making failures invisible to callers
2. **No topic-based routing** — Only two hardcoded message streams (Inbound/Outbound); system events, cron triggers, and tool calls share the same pipes
3. **No ACK mechanism** — Pure fire-and-forget with no way to confirm message processing
4. **No priority differentiation** — Cron, heartbeat, and user messages compete in the same bounded channel
5. **Tight coupling** — Actor lifecycle (Router, Session, Outbound) is tangled with message transport

## 2. Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Architecture | Complete Bus replacement | Clean break from fixed two-channel model |
| Actor model | Abolished | Replaced by Broker subscribe + spawn; SessionManager preserves serial-per-session semantics |
| Topic granularity | Fine-grained with parameters | e.g., `ToolCall(String)` routes to specific tool |
| Channel primitive | `async-channel` (P2P) + `tokio::broadcast` (fanout) | Matches actual delivery semantics per topic |
| Backpressure | Dual-mode API | `publish().await` (blocking) + `try_publish()` (non-blocking) |
| ACK semantics | Optional per-message | Broker-managed side-channel (`DashMap<Uuid, oneshot::Sender>`); Envelope stays pure data, fully `Clone`-safe |
| Crate location | New `gasket-mq` crate | Clean dependency boundary; engine re-exports |

## 3. Message Contract Layer

### 3.1 Topic

Strong-typed enum. Rejects stringly-typed routing.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum Topic {
    // External channel messages
    #[default]
    Inbound,
    Outbound,

    // Internal system events
    SystemEvent,
    ToolCall(String),
    LlmRequest,
    Stream(String),       // session_key as parameter
    CronTrigger,
    Heartbeat,            // Dedicated topic — no longer competes with user messages
    Custom(String),
}
```

**Validation:** `ToolCall("")` and `Custom("")` are rejected at construction time via a `Topic::tool_call(name)` / `Topic::custom(name)` constructor that returns `Result<Topic, BrokerError>` on empty strings. Direct enum construction is still possible but discouraged.

### 3.2 Delivery Mode

Compile-time decision per topic — not a runtime guess.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMode {
    /// Message consumed by exactly one subscriber (work-stealing)
    PointToPoint,
    /// Message delivered to all subscribers
    Broadcast,
}

impl Topic {
    pub fn delivery_mode(&self) -> DeliveryMode {
        match self {
            Topic::SystemEvent => DeliveryMode::Broadcast,
            _ => DeliveryMode::PointToPoint,
        }
    }
}
```

**Rationale:** Only `SystemEvent` needs broadcast (multiple components observe system lifecycle). `Stream(session_key)` is PointToPoint — each streaming session has exactly one WebSocket consumer. All other topics are point-to-point where one consumer handles the message.

### 3.3 Envelope

**Design constraint:** `Envelope` must implement `Clone` (required by `tokio::broadcast`). Therefore, the ACK callback (`oneshot::Sender`) CANNOT live inside the Envelope — it is managed by the Broker as a side-channel.

```rust
/// Pure data envelope — no callbacks, no channels, fully Clone-safe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub id: uuid::Uuid,
    pub timestamp: u64,            // Unix epoch milliseconds
    #[serde(skip, default)]
    pub topic: Topic,              // Topic implements Default (→ Inbound)
    pub payload: serde_json::Value,
}

/// ACK result — only used in the Broker's side-channel, never serialized.
#[derive(Debug)]
pub enum AckResult {
    Ack,
    Nack(String),
}
```

Builder API:
```rust
impl Envelope {
    /// Quick construction — auto-generates ID and timestamp
    pub fn new(topic: Topic, payload: impl Serialize) -> Self { ... }
}
```

**ACK mechanism** is on the Broker, not the Envelope (see §4.1).

## 4. Broker Core Abstraction

### 4.1 MessageBroker Trait

> **Note on `async_trait`:** Rust 1.75+ supports native `async fn in trait`. However, our trait needs `dyn MessageBroker` (object-safety), which requires `#[async_trait]` or the `trait_variant` crate. We use `#[async_trait]` for now; migration to native async is a future optimization.

```rust
#[async_trait]
pub trait MessageBroker: Send + Sync {
    /// Blocking publish — awaits when queue is full (natural backpressure)
    async fn publish(&self, envelope: Envelope) -> Result<(), BrokerError>;

    /// Non-blocking publish — returns QueueFull immediately
    fn try_publish(&self, envelope: Envelope) -> Result<(), BrokerError>;

    /// Publish with ACK — returns a receiver for the consumer's acknowledgment.
    /// The Broker stores the oneshot::Sender in a side-channel keyed by envelope.id.
    /// Consumer calls broker.ack(id) or broker.nack(id, reason) after processing.
    /// Only meaningful for PointToPoint topics (broadcast topics ignore ACK).
    async fn publish_with_ack(&self, envelope: Envelope)
        -> Result<tokio::sync::oneshot::Receiver<AckResult>, BrokerError>;

    /// Acknowledge a message by ID (consumer-side).
    fn ack(&self, id: uuid::Uuid) -> Result<(), BrokerError>;

    /// Negatively acknowledge a message by ID (consumer-side).
    fn nack(&self, id: uuid::Uuid, reason: String) -> Result<(), BrokerError>;

    /// Subscribe to a topic. Returns a unified Subscriber.
    /// - PointToPoint: multiple subscribers share one queue (work-stealing)
    /// - Broadcast: each subscriber gets an independent receiver
    async fn subscribe(&self, topic: &Topic) -> Result<Subscriber, BrokerError>;

    /// Close a topic's queue (graceful shutdown)
    async fn close_topic(&self, topic: &Topic) -> Result<(), BrokerError>;

    /// Queue metrics (depth, total published/consumed)
    fn metrics(&self, topic: &Topic) -> Option<QueueMetrics>;
}
```

### 4.2 Subscriber

Unified receiver that hides the underlying channel type:

```rust
pub enum Subscriber {
    PointToPoint(async_channel::Receiver<Envelope>),
    Broadcast(tokio::sync::broadcast::Receiver<Envelope>),
}

impl Subscriber {
    pub async fn recv(&mut self) -> Result<Envelope, BrokerError> { ... }
}
```

### 4.3 Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error("Queue is full for topic")]
    QueueFull,
    #[error("Channel closed")]
    ChannelClosed,
    #[error("Subscriber lagged behind by {0} messages")]
    Lagged(u64),
    #[error("Topic not found")]
    TopicNotFound,
    #[error("Internal error: {0}")]
    Internal(String),
}
```

## 5. Memory Broker Implementation

### 5.1 MemoryBroker

```rust
pub struct MemoryBroker {
    queues: DashMap<Topic, QueueInner>,
    p2p_capacity: usize,        // default: 1024
    broadcast_capacity: usize,  // default: 256
}
```

Internal queue representation:

```rust
enum QueueInner {
    PointToPoint {
        tx: async_channel::Sender<Envelope>,
        rx: async_channel::Receiver<Envelope>,
        stats: QueueStats,
    },
    Broadcast {
        tx: tokio::sync::broadcast::Sender<Envelope>,
        stats: QueueStats,
    },
}
```

**Key behaviors:**
- **Lazy queue creation:** Queues are created on first `publish` or `subscribe` via `DashMap::entry().or_insert_with()`
- **Fast path:** `DashMap::get()` (read-only, no write lock) for existing topics
- **PointToPoint semantics:** `async_channel::Receiver` is cloneable — multiple consumers do work-stealing
- **Broadcast semantics:** Each `subscribe()` call creates a new `broadcast::Receiver`
- **Atomic stats:** `AtomicU64` for published/consumed counters, `Ordering::Relaxed` (no cross-thread ordering needed for metrics)

### 5.2 Backpressure

| Method | PointToPoint (async-channel) | Broadcast (tokio) |
|--------|-----|-----------|
| `publish().await` | Blocks until space available | Never blocks (broadcast is unbounded-send) |
| `try_publish()` | Returns `QueueFull` if at capacity | Never fails (broadcast send is sync) |

**Note:** `tokio::broadcast` has a fixed-size ring buffer. Slow subscribers receive `BrokerError::Lagged(n)` on next `recv()`, not a send-side error. This is the correct tradeoff for broadcast — we don't want one slow observer to block all publishers.

## 6. Migration: Replacing the Actor Pipeline

### 6.1 Message Flow Comparison

**Before (Actor pipeline):**
```
Channel → bus.publish_inbound() → Router Actor → Session Actor → handler
  → bus.publish_outbound() → Outbound Actor → Channel
```

**After (Broker-based):**
```
Channel → broker.publish(Inbound) → SessionManager.subscribe(Inbound)
  → spawn per-session task → handler → broker.publish(Outbound)
  → OutboundDispatcher.subscribe(Outbound) → Channel
```

### 6.2 SessionManager (replaces Router + Session Actor)

```rust
pub struct SessionManager<H: MessageHandler> {
    broker: Arc<dyn MessageBroker>,
    handler: Arc<H>,
    sessions: DashMap<SessionKey, mpsc::Sender<InboundMessage>>,
    idle_timeout: Duration,
}
```

Preserves from the old design:
- **Serial processing per session** — each session_key gets its own tokio task with a local `mpsc` channel
- **Idle timeout + GC** — tasks self-destruct after inactivity
- **Dead channel respawn** — failed sends trigger fresh task creation

### 6.3 OutboundDispatcher (replaces Outbound Actor)

```rust
pub struct OutboundDispatcher {
    broker: Arc<dyn MessageBroker>,
    registry: Arc<OutboundSenderRegistry>,
}
```

Subscribes to `Topic::Outbound` and dispatches via `tokio::spawn` (fire-and-forget), identical to the original Outbound Actor behavior.

### 6.4 Gateway Initialization

```rust
// Before:
let (bus, inbound_rx, outbound_rx) = MessageBus::new(512);

// After:
let broker = Arc::new(MemoryBroker::default());
let session_mgr = Arc::new(SessionManager::new(broker.clone(), handler, idle_timeout));
tokio::spawn(session_mgr.clone().run());
let outbound = OutboundDispatcher::new(broker.clone(), registry);
tokio::spawn(outbound.run());
```

### 6.5 Channel Integration

All channels currently receive `Sender<InboundMessage>` (or `InboundSender` wrapper). They will instead receive `Arc<dyn MessageBroker>` and call `broker.publish(Envelope::new(Topic::Inbound, msg))`.

## 7. Crate Structure

### 7.1 New Crate: `gasket-mq`

```
gasket/mq/
├── Cargo.toml
└── src/
    ├── lib.rs               # Public API + re-exports
    ├── types.rs             # Topic, Envelope, DeliveryMode, AckResult
    ├── broker.rs            # MessageBroker trait, Subscriber, BrokerError, QueueMetrics
    ├── memory_broker.rs     # MemoryBroker implementation
    ├── session.rs           # SessionManager
    ├── outbound.rs          # OutboundDispatcher
    └── metrics.rs           # Optional Prometheus/tracing export
```

### 7.2 Dependencies

```toml
[dependencies]
async-channel = "2"
async-trait = "0.1"
dashmap = "6"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tokio = { version = "1", features = ["sync", "time", "rt"] }
tracing = "0.1"
uuid = { version = "1", features = ["v4"] }
```

### 7.3 Files to Delete

- `engine/src/bus/mod.rs`
- `engine/src/bus/actors.rs`
- `engine/src/bus/queue.rs`
- `engine/src/bus_adapter.rs`

### 7.4 Files to Modify (~15 files)

| File | Change |
|------|--------|
| `engine/src/lib.rs` | Replace `pub mod bus` with `pub use gasket_mq` |
| `engine/Cargo.toml` | Add `gasket-mq` dependency |
| `cli/src/commands/gateway.rs` | Rewrite initialization (largest change) |
| `engine/src/tools/message.rs` | Use `broker.publish(Topic::Outbound, ...)` |
| `channels/src/telegram.rs` | Replace `Sender<InboundMessage>` with `Arc<dyn MessageBroker>` |
| `channels/src/discord.rs` | Same pattern |
| `channels/src/slack.rs` | Same pattern |
| `channels/src/dingtalk/channel.rs` | Same pattern |
| `channels/src/dingtalk/webhook.rs` | Same pattern |
| `channels/src/feishu/channel.rs` | Same pattern |
| `channels/src/feishu/webhook.rs` | Same pattern |
| `channels/src/wecom/channel.rs` | Same pattern |
| `channels/src/wecom/webhook.rs` | Same pattern |
| `channels/src/websocket.rs` | Same pattern |
| `channels/src/lib.rs` | Update `InboundSender` or remove |

## 8. Future: SQLite Persistent Broker

Reserved for topics that must survive process crashes (e.g., `CronTrigger`):

```rust
pub struct SqliteBroker {
    memory: MemoryBroker,
    db: sqlx::SqlitePool,
    persistent_topics: HashSet<Topic>,
}
```

Schema: `CREATE TABLE queue (id TEXT PRIMARY KEY, topic TEXT, payload TEXT, status INTEGER)`

**NOT in scope for initial implementation.** Only build when there is a concrete durability requirement.

## 9. Testing Strategy

1. **Unit tests** for `MemoryBroker`:
   - Backpressure: 10,000 messages into capacity-1000 queue → `try_publish` returns `QueueFull` after 1000
   - Work-stealing: 2 consumers on same P2P topic → each gets ~half
   - Broadcast: 2 subscribers → both receive all messages
   - Lagged subscriber detection
   - ACK round-trip via `with_ack()`

2. **Integration tests** for `SessionManager`:
   - Inbound message → correct session task → outbound published
   - Idle timeout → task cleanup
   - Dead session respawn

3. **Migration validation**:
   - Full pipeline: Channel → Inbound → SessionManager → handler → Outbound → Channel
   - Streaming (WebSocket): Channel → Inbound → SessionManager → streaming handler → Stream topic → OutboundDispatcher
