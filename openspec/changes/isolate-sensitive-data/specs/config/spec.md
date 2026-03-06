## ADDED Requirements

### Requirement: Vault Reference Resolution
The config loader SHALL recognize and resolve `ref:` prefixed values from the vault.

#### Scenario: Config loading with vault references
- **WHEN** config.yaml contains `api_key: "ref:copilot-token"`
- **THEN** the loader retrieves the actual value from unlocked vault
- **AND** the resolved value is only held in memory, not written back

#### Scenario: Vault locked error
- **WHEN** vault is locked during config load
- **THEN** loader returns error with message "Vault is locked. Run `nanobot vault unlock`"
- **AND** config loading fails gracefully

#### Scenario: Backward compatibility
- **WHEN** config.yaml contains plain text value (no `ref:` prefix)
- **THEN** loader uses the value directly without vault lookup
- **AND** no error is raised

## MODIFIED Requirements

### Requirement: Config Debug Output
The config Debug trait SHALL always redact resolved vault references.

#### Scenario: Debug printing with resolved refs
- **WHEN** config is printed with `{:?}`
- **THEN** all `api_key` fields show `***REDACTED***` regardless of source
- **AND** vault-resolved values are never exposed in logs
