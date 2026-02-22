# Provider Registry Specification

## MODIFIED Requirements

### Requirement: LLM Provider Trait
The system SHALL provide an `LlmProvider` trait that accepts `TrailContext` for observability.

#### Scenario: Chat with trail context
- **WHEN** `provider.chat(request, trail_ctx)` is called
- **THEN** the request SHALL be processed with trail tracking
- **AND** spans SHALL be created for the LLM API call
- **AND** the `TrailContext` SHALL be propagated to middleware

#### Scenario: Provider metadata
- **WHEN** `provider.metadata()` is called
- **THEN** provider information (name, default model, capabilities) SHALL be returned
- **AND** the metadata SHALL be available without initialization

## ADDED Requirements

### Requirement: Provider Middleware
The system SHALL support middleware for intercepting provider requests and responses.

#### Scenario: Request logging
- **WHEN** a `LoggingMiddleware` is registered
- **THEN** all provider requests SHALL be logged with trail context
- **AND** request/response timing SHALL be recorded

#### Scenario: Retry on failure
- **WHEN** a `RetryMiddleware` is configured with max_attempts=3
- **AND** a provider request fails
- **THEN** the request SHALL be retried up to 3 times with exponential backoff
- **AND** retry attempts SHALL be recorded in the trail

#### Scenario: Metrics collection
- **WHEN** a `MetricsMiddleware` is registered
- **THEN** the following metrics SHALL be collected:
  - Request latency
  - Token usage (prompt + completion)
  - Error rate by error type
  - Provider-specific metrics

### Requirement: Provider Builder
The system SHALL provide a Builder pattern for configuring providers.

#### Scenario: Builder configuration
- **WHEN** creating a provider via builder
```rust
let provider = OpenAIProvider::builder()
    .api_key(key)
    .model("gpt-4")
    .middleware(LoggingMiddleware::new())
    .middleware(RetryMiddleware::new(3))
    .trail(trail)
    .build()?
```
- **THEN** the provider SHALL be configured with all specified options
- **AND** middleware SHALL execute in the specified order
- **AND** validation errors SHALL be returned before initialization

#### Scenario: Default configuration
- **WHEN** `OpenAIProvider::builder().api_key(key).build()` is called
- **THEN** sensible defaults SHALL be used for model and other settings
- **AND** no middleware SHALL be registered by default

### Requirement: Provider Registry Management
The system SHALL provide enhanced registry capabilities for dynamic provider management.

#### Scenario: Dynamic registration
- **WHEN** `registry.register("custom", provider)` is called
- **THEN** the provider SHALL be available for use immediately
- **AND** the provider SHALL be discoverable via `registry.get("custom")`

#### Scenario: Provider discovery
- **WHEN** `registry.list()` is called
- **THEN** all registered provider names SHALL be returned
- **AND** each provider's metadata SHALL be accessible

#### Scenario: Default provider
- **WHEN** `registry.get_default()` is called
- **THEN** the default provider SHALL be returned
- **AND** if no default is set, an error SHALL be returned
