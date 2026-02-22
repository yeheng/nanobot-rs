# Tool Registry Specification

## MODIFIED Requirements

### Requirement: Tool Trait
The system SHALL provide a `Tool` trait that accepts `ExecutionContext` with trail information.

#### Scenario: Tool execution with context
- **WHEN** `tool.execute(args, ctx)` is called
- **THEN** the tool SHALL execute with access to:
  - `TrailContext` for recording operations
  - `ToolMetadata` for capability information
  - `Permission` for security checks
- **AND** tool execution SHALL be tracked in the trail

## ADDED Requirements

### Requirement: Tool Middleware
The system SHALL support middleware for tool execution interception.

#### Scenario: Permission checking
- **WHEN** a `PermissionMiddleware` is registered
- **AND** a tool requires "filesystem:write" permission
- **AND** the context lacks this permission
- **THEN** execution SHALL be denied
- **AND** the denial SHALL be logged with reason

#### Scenario: Execution timeout
- **WHEN** a `TimeoutMiddleware` is configured with 30s timeout
- **AND** a tool execution exceeds 30s
- **THEN** the execution SHALL be cancelled
- **AND** a timeout error SHALL be returned

#### Scenario: Result caching
- **WHEN** a `CachingMiddleware` is registered
- **AND** a tool is called with identical arguments
- **THEN** the cached result MAY be returned
- **AND** cache hit/miss SHALL be recorded in the trail

### Requirement: Tool Metadata
The system SHALL provide rich metadata for tool discovery and security.

#### Scenario: Metadata definition
- **WHEN** a tool is implemented
- **THEN** it SHALL provide `ToolMetadata` containing:
  - Name and description
  - Parameter schema (JSON Schema)
  - Required permissions
  - Tags for categorization
  - Examples of usage

#### Scenario: Registry query by tag
- **WHEN** `registry.find_by_tag("filesystem")` is called
- **THEN** all tools with the "filesystem" tag SHALL be returned
- **AND** the result SHALL be ordered alphabetically by tool name

#### Scenario: Permission discovery
- **WHEN** `registry.get_all_permissions()` is called
- **THEN** a list of all permissions required by registered tools SHALL be returned
- **AND** this SHALL be used for agent permission configuration

### Requirement: Tool Registry Enhancements
The system SHALL provide enhanced registry capabilities.

#### Scenario: Tool categorization
- **WHEN** tools are registered
- **THEN** they SHALL be automatically categorized by type:
  - Filesystem tools (read, write, list)
  - Network tools (http, websocket)
  - System tools (shell, process)
  - Communication tools (message, notify)

#### Scenario: Tool discovery for LLM
- **WHEN** `registry.get_definitions()` is called
- **THEN** tool definitions SHALL be returned in OpenAI-compatible format
- **AND** definitions SHALL include all metadata needed for LLM function calling

#### Scenario: Hot reload (optional)
- **WHEN** a tool is re-registered with the same name
- **THEN** the new implementation SHALL replace the old one
- **AND** in-flight executions SHALL complete with the old implementation
