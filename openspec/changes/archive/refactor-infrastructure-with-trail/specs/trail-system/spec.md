# Trail System Specification

## ADDED Requirements

### Requirement: Trail Core Trait
The system SHALL provide a `Trail` trait for recording execution traces with hierarchical spans and events.

#### Scenario: Start and end span
- **WHEN** a component calls `trail.start_span("operation", attrs)`
- **THEN** a new span SHALL be created with a unique `SpanId`
- **AND** the span SHALL be tracked in the current context
- **WHEN** `trail.end_span(span_id)` is called
- **THEN** the span SHALL be closed and its duration recorded

#### Scenario: Record events
- **WHEN** a component calls `trail.record_event("event_name", attrs)`
- **THEN** an event SHALL be recorded with timestamp and attributes
- **AND** the event SHALL be associated with the current span

#### Scenario: Context propagation
- **WHEN** `trail.current_context()` is called
- **THEN** a `TrailContext` SHALL be returned containing:
  - `trace_id`: Unique identifier for the entire trace
  - `span_id`: Current span identifier
  - `baggage`: Key-value pairs for cross-component data

### Requirement: Trail Context Propagation
The system SHALL support asynchronous context propagation across async boundaries.

#### Scenario: Async context preservation
- **WHEN** an async task is spawned with a `TrailContext`
- **THEN** the context SHALL be available in the spawned task
- **AND** new spans created in the task SHALL be children of the parent span

#### Scenario: Cross-component tracing
- **WHEN** an operation spans multiple components (provider → tool → memory)
- **THEN** all spans SHALL share the same `trace_id`
- **AND** the parent-child relationship SHALL be maintained

### Requirement: Built-in Trail Implementations
The system SHALL provide built-in implementations for common use cases.

#### Scenario: DefaultTrail
- **WHEN** `DefaultTrail::new()` is used
- **THEN** spans and events SHALL be stored in memory
- **AND** the trail data SHALL be queryable for visualization

#### Scenario: NoopTrail
- **WHEN** `NoopTrail` is used
- **THEN** all trail operations SHALL be no-ops
- **AND** minimal performance overhead SHALL be incurred

### Requirement: Span Tree Visualization
The system SHALL provide utilities for visualizing span hierarchies.

#### Scenario: Export to JSON
- **WHEN** `trail.to_json()` is called
- **THEN** the complete span tree SHALL be exported as JSON
- **AND** the JSON SHALL include timing, attributes, and events for each span

#### Scenario: Text representation
- **WHEN** `trail.to_string()` is called
- **THEN** a human-readable hierarchical tree SHALL be produced
- **AND** each level SHALL be indented to show nesting

## ADDED Requirements

### Requirement: Middleware Infrastructure
The system SHALL provide a generic middleware pattern for cross-cutting concerns.

#### Scenario: Middleware chaining
- **WHEN** multiple middlewares are registered
- **THEN** they SHALL execute in the order they were added (outer to inner)
- **AND** the `Next` type SHALL allow delegating to the next middleware

#### Scenario: Middleware short-circuit
- **WHEN** a middleware returns early without calling `next`
- **THEN** subsequent middlewares SHALL NOT be executed
- **AND** the response SHALL be returned to the caller

#### Scenario: Middleware error handling
- **WHEN** a middleware encounters an error
- **THEN** the error SHALL propagate to outer middlewares
- **AND** outer middlewares MAY handle or re-throw the error
