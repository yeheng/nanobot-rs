## ADDED Requirements

### Requirement: Stateless Hook Contract

All implementations of the `AgentHook` trait SHALL be **stateless** with respect to cross-request mutable state. Hooks MUST NOT maintain mutable state that is shared or accessed across different `process_direct` invocations.

Acceptable state within a hook:
- **Read-only configuration** loaded at construction time (e.g., system prompt text, skills context)
- **Service references** to external stores (e.g., `SessionManager`, `SqliteStore`) that manage their own concurrency via connection pools
- **Metrics counters** using lock-free atomics (`AtomicU32`, etc.)

Prohibited state within a hook:
- **Mutable collections** keyed by session or request (e.g., `Mutex<HashMap<String, Session>>`)
- **Any in-memory cache** that acts as a source of truth for request-scoped data

#### Scenario: Hook receives all necessary context via Context structs
- **WHEN** a hook's lifecycle method is called (e.g., `on_session_save`)
- **THEN** all information needed to perform its work SHALL be available through the `*Context` struct parameter and the hook's immutable configuration
- **AND** the hook SHALL NOT need to maintain or consult internal mutable state keyed by session or request

#### Scenario: PersistenceHook operates without in-memory session cache
- **WHEN** `PersistenceHook::on_session_save` is called
- **THEN** it SHALL persist the message directly to SQLite via `SessionManager::append_message` without consulting an in-memory session cache
- **AND** `PersistenceHook` SHALL NOT hold any `Mutex<HashMap<String, Session>>` or equivalent mutable collection

### Requirement: Hook Sequential Execution Within Request

All registered hooks SHALL be invoked **sequentially** (in registration order) at each lifecycle stage. The system SHALL NOT invoke multiple hooks concurrently within a single lifecycle stage of a single request.

#### Scenario: Three hooks registered for on_request
- **WHEN** hooks A, B, C are registered in order
- **AND** `on_request` is called
- **THEN** hook A's `on_request` SHALL complete before hook B's `on_request` begins
- **AND** hook B's `on_request` SHALL complete before hook C's `on_request` begins

#### Scenario: Early termination via skip flag
- **WHEN** hook A sets `ctx.skip = true` during `on_request`
- **THEN** hooks B and C SHALL NOT have their `on_request` called for this request
