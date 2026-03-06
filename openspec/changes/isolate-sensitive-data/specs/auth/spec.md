## MODIFIED Requirements

### Requirement: OAuth/PAT Token Storage
The auth command SHALL store tokens in the vault instead of config.yaml.

#### Scenario: Copilot PAT authentication
- **WHEN** user runs `nanobot auth copilot --pat <token>`
- **THEN** token is stored in vault with key `copilot-token`
- **AND** config.yaml receives `api_key: "ref:copilot-token"`
- **AND** original token is never written to config file

#### Scenario: Copilot OAuth device flow
- **WHEN** user completes OAuth device flow
- **THEN** the received access token is stored in vault
- **AND** config.yaml receives `api_key: "ref:copilot-token"`

#### Scenario: Vault locked during auth
- **WHEN** vault is locked during auth command
- **THEN** user is prompted to unlock vault first
- **AND** auth fails with helpful error message
