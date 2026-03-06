## MODIFIED Requirements

### Requirement: History Privacy
The history processor SHALL never store resolved vault references.

#### Scenario: Conversation history with config access
- **WHEN** agent accesses config during conversation
- **THEN** only `ref:xxx` strings are visible to agent
- **AND** resolved values are never added to history
- **AND** resolved values are never written to memory files

## ADDED Requirements

### Requirement: Memory Module Isolation
The memory system SHALL maintain strict isolation between sensitive and non-sensitive data.

#### Scenario: Memory search with sensitive keywords
- **WHEN** user searches memory for token-related content
- **THEN** only `ref:` references are returned
- **AND** actual credentials are never exposed via memory APIs
