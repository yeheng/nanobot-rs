## ADDED Requirements

### Requirement: Rust Core Implementation

nanobot core SHALL be implemented in Rust, providing a lightweight AI assistant framework with the following characteristics:

- Total lines of code SHALL NOT exceed 10,000 (excluding tests and generated code)
- Binary size SHALL be less than 20MB (release build, stripped)
- Startup time SHALL be less than 100ms
- Memory footprint SHALL be less than 50MB during idle operation

#### Scenario: Core performance benchmarks
- **WHEN** the nanobot binary is started
- **THEN** it SHALL initialize within 100ms
- **AND** the memory footprint SHALL be less than 50MB

#### Scenario: Binary distribution
- **WHEN** a release build is compiled with `--release`
- **THEN** the resulting binary SHALL be statically linked where possible
- **AND** the binary SHALL run without any runtime dependencies

---

### Requirement: Backward Compatible Configuration

The Rust implementation SHALL maintain full backward compatibility with existing Python configuration format.

- Configuration file location: `~/.nanobot/config.yaml`
- All existing configuration fields SHALL be supported
- New fields MAY be added with sensible defaults

#### Scenario: Load existing Python configuration
- **GIVEN** a configuration file created by Python nanobot v0.1.3
- **WHEN** the Rust implementation loads the configuration
- **THEN** all providers, agents, and channels SHALL be correctly parsed
- **AND** no migration or modification SHALL be required

#### Scenario: Configuration validation
- **WHEN** an invalid configuration is loaded
- **THEN** a clear error message SHALL be displayed
- **AND** the error SHALL indicate the specific field and issue

---

### Requirement: Backward Compatible CLI

The Rust implementation SHALL provide a CLI interface compatible with the Python version.

#### Scenario: Basic CLI commands
- **WHEN** user runs `nanobot --version`
- **THEN** the version SHALL be displayed in format `nanobot v2.0.0`

- **WHEN** user runs `nanobot onboard`
- **THEN** the configuration wizard SHALL initialize `~/.nanobot/`

- **WHEN** user runs `nanobot status`
- **THEN** provider and channel status SHALL be displayed

#### Scenario: Agent command
- **WHEN** user runs `nanobot agent -m "Hello"`
- **THEN** the agent SHALL respond appropriately
- **AND** the conversation SHALL be saved to session

- **WHEN** user runs `nanobot agent` without `-m`
- **THEN** interactive REPL mode SHALL start
- **AND** user can type messages and receive responses
- **AND** `/new`, `/help`, `/exit` commands SHALL work

---

### Requirement: LLM Provider Support

The Rust implementation SHALL support multiple LLM providers via OpenAI-compatible API.

#### Scenario: OpenAI provider
- **GIVEN** valid OpenAI API key in configuration
- **WHEN** user sends a message
- **THEN** the agent SHALL respond using OpenAI models

#### Scenario: OpenRouter provider
- **GIVEN** valid OpenRouter API key in configuration
- **WHEN** user specifies a model like `anthropic/claude-opus-4-5`
- **THEN** the agent SHALL route the request through OpenRouter

#### Scenario: Anthropic direct
- **GIVEN** valid Anthropic API key in configuration
- **WHEN** user specifies an Anthropic model
- **THEN** the agent SHALL use Anthropic's native API

---

### Requirement: Tool System

The Rust implementation SHALL provide a tool system with the same capabilities as the Python version.

#### Scenario: File operations
- **WHEN** the agent uses `read_file` tool
- **THEN** file contents SHALL be returned
- **AND** path traversal SHALL be prevented when `restrictToWorkspace` is enabled

- **WHEN** the agent uses `write_file` tool
- **THEN** the file SHALL be created or overwritten
- **AND** parent directories SHALL be created if needed

- **WHEN** the agent uses `edit_file` tool
- **THEN** targeted string replacement SHALL be performed
- **AND** the operation SHALL fail if `old_string` is not unique

#### Scenario: Shell execution
- **WHEN** the agent uses `exec` tool
- **THEN** the command SHALL execute in the workspace directory
- **AND** timeout SHALL be enforced
- **AND** `restrictToWorkspace` SHALL restrict file access

#### Scenario: Web tools
- **WHEN** the agent uses `web_search` tool with valid Brave API key
- **THEN** search results SHALL be returned

- **WHEN** the agent uses `web_fetch` tool
- **THEN** web page content SHALL be extracted and summarized

---

### Requirement: Channel Support

The Rust implementation SHALL support multiple chat channels.

#### Scenario: Telegram channel
- **GIVEN** valid Telegram bot token in configuration
- **WHEN** user sends a message to the bot
- **THEN** the agent SHALL respond in the same chat
- **AND** `allowFrom` whitelist SHALL be enforced

#### Scenario: Discord channel
- **GIVEN** valid Discord bot token with MESSAGE CONTENT INTENT
- **WHEN** user sends a DM or mentions the bot in a channel
- **THEN** the agent SHALL respond appropriately

#### Scenario: Slack channel
- **GIVEN** valid Slack bot token and app token (Socket Mode)
- **WHEN** user DMs or @mentions the bot
- **THEN** the agent SHALL respond
- **AND** `groupPolicy` (mention/open/allowlist) SHALL be respected

#### Scenario: Email channel
- **GIVEN** valid IMAP and SMTP credentials
- **WHEN** an email is received from an allowed sender
- **THEN** the agent SHALL process and reply

---

### Requirement: Session Management

The Rust implementation SHALL maintain conversation sessions compatible with the Python version.

#### Scenario: Session persistence
- **WHEN** a conversation occurs
- **THEN** messages SHALL be saved to `~/.nanobot/sessions/`
- **AND** the format SHALL be compatible with Python version

#### Scenario: Session continuity
- **GIVEN** an existing session from Python version
- **WHEN** user continues the conversation in Rust version
- **THEN** previous context SHALL be available

---

### Requirement: Memory System

The Rust implementation SHALL provide a memory system for long-term context retention.

#### Scenario: Long-term memory
- **WHEN** the agent learns important facts about the user
- **THEN** they SHALL be stored in `~/.nanobot/memory/MEMORY.md`

#### Scenario: Memory consolidation
- **WHEN** a session exceeds the memory window
- **THEN** old messages SHALL be consolidated via LLM
- **AND** a summary SHALL be appended to `HISTORY.md`

---

### Requirement: Scheduled Tasks (Cron)

The Rust implementation SHALL support scheduled task execution.

#### Scenario: Add cron job
- **WHEN** user runs `nanobot cron add --name "daily" --cron "0 9 * * *" --message "Good morning!"`
- **THEN** the job SHALL be scheduled for 9 AM daily

#### Scenario: List cron jobs
- **WHEN** user runs `nanobot cron list`
- **THEN** all scheduled jobs SHALL be displayed with their schedules

#### Scenario: Remove cron job
- **WHEN** user runs `nanobot cron remove <job_id>`
- **THEN** the job SHALL be removed from the schedule

---

### Requirement: MCP Support

The Rust implementation SHALL support Model Context Protocol (MCP) for external tool integration.

#### Scenario: Stdio MCP server
- **GIVEN** an MCP server configured with `command` and `args`
- **WHEN** the agent starts
- **THEN** the MCP server SHALL be launched
- **AND** its tools SHALL be available to the agent

#### Scenario: HTTP MCP server
- **GIVEN** an MCP server configured with `url`
- **WHEN** the agent starts
- **THEN** the HTTP connection SHALL be established
- **AND** server tools SHALL be registered

---

### Requirement: Error Handling

The Rust implementation SHALL provide clear error messages and graceful degradation.

#### Scenario: API key missing
- **WHEN** a required API key is not configured
- **THEN** a clear error SHALL be displayed with instructions to configure

#### Scenario: Network error
- **WHEN** an LLM API request fails due to network issues
- **THEN** a retry with exponential backoff SHALL be attempted
- **AND** a clear error SHALL be shown after max retries

#### Scenario: Channel disconnection
- **WHEN** a chat channel disconnects
- **THEN** automatic reconnection SHALL be attempted
- **AND** other channels SHALL continue operating
