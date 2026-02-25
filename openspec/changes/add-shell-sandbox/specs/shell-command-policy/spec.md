## ADDED Requirements

### Requirement: Command Denylist
The system SHALL support a configurable list of denied command patterns. When a command matches a denylist pattern, execution MUST be rejected with an error message before any process is spawned. Denylist patterns SHALL be matched against the raw command string using substring or glob matching.

#### Scenario: Command matches denylist pattern
- **WHEN** a command string matches any pattern in the configured denylist
- **THEN** the system SHALL reject the command with an error message indicating which denylist rule was triggered, and SHALL NOT execute the command

#### Scenario: Empty denylist
- **WHEN** no denylist patterns are configured
- **THEN** the system SHALL allow all commands (subject to other security controls)

### Requirement: Command Allowlist
The system SHALL support a configurable list of allowed command patterns. When an allowlist is configured and non-empty, ONLY commands matching an allowlist pattern SHALL be permitted. Allowlist patterns SHALL be matched against the command binary name (the first token of the command string).

#### Scenario: Command matches allowlist
- **WHEN** an allowlist is configured AND the command binary matches an allowlist entry
- **THEN** the system SHALL permit the command to proceed to execution

#### Scenario: Command does not match allowlist
- **WHEN** an allowlist is configured AND non-empty AND the command binary does NOT match any allowlist entry
- **THEN** the system SHALL reject the command with an error message listing the allowed commands

#### Scenario: No allowlist configured
- **WHEN** no allowlist is configured or the allowlist is empty
- **THEN** the system SHALL allow all commands (subject to denylist and other security controls)

### Requirement: Command Policy Evaluation Order
The system SHALL evaluate command policies in the following order: (1) allowlist check, (2) denylist check. A command MUST pass both checks to proceed to execution. Policy violations SHALL be logged at warn level with the full command string.

#### Scenario: Policy evaluation order
- **WHEN** both allowlist and denylist are configured AND a command matches the allowlist but also matches a denylist pattern
- **THEN** the system SHALL reject the command (denylist takes precedence after allowlist pass)

#### Scenario: Policy audit logging
- **WHEN** a command is rejected by any policy rule
- **THEN** the system SHALL log a warning containing the rejected command and the rule that triggered the rejection
