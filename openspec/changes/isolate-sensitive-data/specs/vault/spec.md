## ADDED Requirements

### Requirement: Vault Storage
The system SHALL provide encrypted storage for sensitive credentials.

#### Scenario: Vault initialization
- **WHEN** user stores first credential
- **THEN** vault file is created at `~/.nanobot/vault/credentials.enc`
- **AND** user is prompted to set a master password
- **AND** data is encrypted with AES-GCM-256

#### Scenario: Store credential
- **WHEN** user runs `nanobot vault set <key> <value>`
- **THEN** value is encrypted and stored with key
- **AND** plaintext value is zeroed from memory

### Requirement: Vault Unlock
The system SHALL require vault unlock before accessing sensitive data.

#### Scenario: Interactive unlock
- **WHEN** user runs `nanobot vault unlock`
- **THEN** user is prompted for master password
- **AND** vault is decrypted and held in memory for the session

#### Scenario: Auto-lock on exit
- **WHEN** nanobot process exits
- **THEN** vault memory is zeroed before termination

### Requirement: Memory Protection
The vault SHALL protect decrypted credentials from memory analysis.

#### Scenario: Swap prevention
- **WHEN** vault is unlocked
- **THEN** decrypted data is locked in memory (mlock)
- **AND** OS cannot swap it to disk

#### Scenario: Secure cleanup
- **WHEN** vault is locked or process exits
- **THEN** all decrypted buffers are zeroed before drop

### Requirement: CLI Management Commands
The system SHALL provide CLI commands for vault management.

#### Scenario: List stored keys
- **WHEN** user runs `nanobot vault list`
- **THEN** all stored key names are displayed
- **AND** no values are shown

#### Scenario: Check vault status
- **WHEN** user runs `nanobot vault status`
- **THEN** output shows whether vault is unlocked
- **AND** shows number of stored credentials

## ADDED Requirements (Keychain Integration)

### Requirement: Optional OS Keychain Backend
The vault MAY use macOS Keychain or Windows Credential Manager as backend.

#### Scenario: macOS keychain storage
- **WHEN** user enables keychain backend on macOS
- **THEN** credentials are stored in system keychain
- **AND** file-based vault is not used
