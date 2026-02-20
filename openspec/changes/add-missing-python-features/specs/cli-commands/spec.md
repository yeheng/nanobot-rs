## ADDED Requirements

### Requirement: Channels Status Command

The system SHALL provide a CLI command to display channel status.

#### Scenario: Display all channel status

- **WHEN** user runs `nanobot channels status`
- **THEN** the system SHALL display a table with columns:
  - Channel name
  - Status (enabled/disabled)
  - Connection status (connected/disconnected/not applicable)
  - Credential status (✓ configured / ✗ not configured)

#### Scenario: Show configured channels only

- **WHEN** displaying channel status
- **THEN** the system SHALL only show channels that are configured in config.yaml
- **AND** indicate which channels are available but not configured

#### Scenario: Show credential availability

- **WHEN** a channel requires API credentials
- **THEN** the system SHALL check if credentials are configured
- **AND** display ✓ if API key is set
- **AND** display ✗ if API key is missing

#### Scenario: Show feature flag availability

- **WHEN** a channel requires a feature flag (e.g., feishu, dingtalk)
- **THEN** the system SHALL check if the feature is compiled
- **AND** display "Available" if compiled
- **AND** display "Not compiled" if not compiled

### Requirement: Channels Login Command

The system SHALL provide a CLI command for channel authentication.

#### Scenario: WhatsApp QR login

- **WHEN** user runs `nanobot channels login --channel whatsapp`
- **THEN** the system SHALL connect to WhatsApp bridge
- **AND** display QR code in terminal
- **AND** wait for authentication
- **AND** notify user when successfully authenticated

#### Scenario: Login for non-QR channel

- **WHEN** user runs `nanobot channels login` for a channel that doesn't require QR login
- **THEN** the system SHALL display message: "This channel doesn't require login via CLI"
- **AND** show configuration instructions

### Requirement: Cron List Command

The system SHALL provide a CLI command to list scheduled tasks.

#### Scenario: List all cron jobs

- **WHEN** user runs `nanobot cron list`
- **THEN** the system SHALL display a table with columns:
  - Task ID
  - Task name
  - Cron schedule
  - Next run time
  - Status (enabled/disabled)
  - Last run result (success/failed/never)

#### Scenario: Empty cron list

- **WHEN** no cron jobs are configured
- **THEN** the system SHALL display: "No scheduled tasks found"
- **AND** suggest: "Use 'nanobot cron add' to create a task"

#### Scenario: Format cron schedule

- **WHEN** displaying cron schedule
- **THEN** the system SHALL show the cron expression
- **AND** optionally show human-readable description (e.g., "Every day at 9:00 AM")

### Requirement: Cron Add Command

The system SHALL provide a CLI command to add a scheduled task.

#### Scenario: Add cron task with all parameters

- **WHEN** user runs:
  ```bash
  nanobot cron add \
    --name "daily-reminder" \
    --schedule "0 9 * * *" \
    --message "Good morning! Time to start work." \
    --channel "telegram" \
    --chat-id "123456"
  ```
- **THEN** the system SHALL validate the cron expression
- **AND** create a new cron job
- **AND** persist the job to `~/.nanobot/cron/jobs.json`
- **AND** display: "Task created with ID: <task-id>"
- **AND** show next run time

#### Scenario: Add one-time task

- **WHEN** user runs:
  ```bash
  nanobot cron add \
    --name "meeting-reminder" \
    --at "2024-12-25 10:00" \
    --message "Meeting in 1 hour"
  ```
- **THEN** the system SHALL create a one-time task
- **AND** calculate and display the execution time

#### Scenario: Invalid cron expression

- **WHEN** user provides an invalid cron expression
- **THEN** the system SHALL display error: "Invalid cron expression: <expression>"
- **AND** show cron format help

#### Scenario: Missing required parameters

- **WHEN** user omits required parameters (name, schedule/at, message)
- **THEN** the system SHALL display error with missing parameter
- **AND** show command usage

### Requirement: Cron Remove Command

The system SHALL provide a CLI command to remove a scheduled task.

#### Scenario: Remove existing task

- **WHEN** user runs `nanobot cron remove --id <task-id>`
- **THEN** the system SHALL display confirmation prompt:
  "Remove task '<task-name>' (ID: <task-id>)? [y/N]"
- **AND** require user confirmation
- **AND** remove the task if confirmed
- **AND** display: "Task removed successfully"

#### Scenario: Remove non-existent task

- **WHEN** user tries to remove a task that doesn't exist
- **THEN** the system SHALL display error: "Task not found: <task-id>"

#### Scenario: Skip confirmation with flag

- **WHEN** user runs `nanobot cron remove --id <task-id> --yes`
- **THEN** the system SHALL remove the task without confirmation

### Requirement: Cron Enable/Disable Command

The system SHALL provide CLI commands to enable or disable tasks.

#### Scenario: Disable running task

- **WHEN** user runs `nanobot cron disable --id <task-id>`
- **THEN** the system SHALL mark the task as disabled
- **AND** persist the change
- **AND** display: "Task '<task-name>' disabled"

#### Scenario: Enable disabled task

- **WHEN** user runs `nanobot cron enable --id <task-id>`
- **THEN** the system SHALL mark the task as enabled
- **AND** calculate next run time
- **AND** display: "Task '<task-name>' enabled. Next run: <time>"

#### Scenario: Toggle already enabled/disabled task

- **WHEN** user enables an already enabled task
- **THEN** the system SHALL display: "Task is already enabled"

### Requirement: Cron Run Command

The system SHALL provide a CLI command to manually trigger a task.

#### Scenario: Run task immediately

- **WHEN** user runs `nanobot cron run --id <task-id>`
- **THEN** the system SHALL execute the task immediately
- **AND** display execution progress
- **AND** show result (success/failed)
- **AND** display task output if applicable

#### Scenario: Run disabled task

- **WHEN** user tries to run a disabled task
- **THEN** the system SHALL display warning: "Task is disabled. Run anyway? [y/N]"
- **AND** proceed if user confirms

#### Scenario: Run task with channel not available

- **WHEN** the task's target channel is not connected
- **THEN** the system SHALL display error: "Channel '<channel>' is not available"
- **AND** suggest: "Start the gateway with 'nanobot gateway'"

### Requirement: Message Tool Integration

The system SHALL provide a message tool for agents to send messages.

#### Scenario: Send message to specific channel

- **WHEN** the agent uses the message tool with parameters:
  ```json
  {
    "channel": "telegram",
    "chat_id": "123456",
    "content": "Hello from agent!"
  }
  ```
- **THEN** the system SHALL create an OutboundMessage
- **AND** publish it to the message bus
- **AND** route to the specified channel
- **AND** return success to the agent

#### Scenario: Send message to non-existent channel

- **WHEN** the agent tries to send to a channel that's not configured
- **THEN** the system SHALL return error: "Channel '<channel>' not found or not enabled"

#### Scenario: Send message to disconnected channel

- **WHEN** the agent tries to send to a disconnected channel
- **THEN** the system SHALL queue the message
- **AND** return: "Message queued (channel temporarily disconnected)"

### Requirement: Transcription Service

The system SHALL provide audio transcription capabilities.

#### Scenario: Configure Groq transcription

- **WHEN** the configuration includes:
  ```json
  {
    "tools": {
      "transcription": {
        "enabled": true,
        "provider": "groq",
        "language": "auto"
      }
    }
  }
  ```
- **THEN** the system SHALL initialize Groq transcription service
- **AND** make it available for audio messages

#### Scenario: Auto-transcribe Telegram voice message

- **WHEN** a Telegram voice message is received
- **AND** transcription is enabled
- **THEN** the system SHALL download the voice file
- **AND** send to Groq Whisper API
- **AND** prepend transcribed text to message content
- **AND** mark it as "[Transcribed voice message]"

#### Scenario: Transcription failure

- **WHEN** transcription API call fails
- **THEN** the system SHALL log the error
- **AND** continue processing the message without transcription
- **AND** add note: "[Voice message - transcription failed]"

#### Scenario: Transcription disabled

- **WHEN** transcription is disabled in config
- **THEN** the system SHALL process audio messages as-is
- **AND** not attempt transcription

### Requirement: Subagent Manager

The system SHALL provide a subagent manager for background tasks.

#### Scenario: Spawn background task

- **WHEN** the agent uses spawn tool with task description
- **THEN** the system SHALL create a subagent task ID
- **AND** execute the task in background
- **AND** return task ID to the agent

#### Scenario: Track task status

- **WHEN** the agent queries task status
- **THEN** the system SHALL return:
  - Task ID
  - Status (pending/running/completed/failed)
  - Start time
  - End time (if completed)
  - Result (if completed)

#### Scenario: Notify on task completion

- **WHEN** a background task completes
- **THEN** the system SHALL send notification to main agent
- **AND** include task ID and result
- **AND** make result available for query

#### Scenario: Task timeout

- **WHEN** a background task exceeds timeout (default 5 minutes)
- **THEN** the system SHALL terminate the task
- **AND** mark status as "failed"
- **AND** set error: "Task timeout"

### Requirement: CLI Output Formatting

The system SHALL provide consistent output formatting for all commands.

#### Scenario: Table output for list commands

- **WHEN** displaying list data (channels, cron jobs)
- **THEN** the system SHALL use table format with aligned columns
- **AND** use terminal width for column sizing
- **AND** support `--format json` for machine-readable output

#### Scenario: Color output

- **WHEN** terminal supports colors
- **THEN** the system SHALL use colors for:
  - Success messages (green)
  - Errors (red)
  - Warnings (yellow)
  - Status indicators (✓ green, ✗ red)

#### Scenario: Disable colors

- **WHEN** output is redirected to file or `--no-color` flag is used
- **THEN** the system SHALL disable color output

#### Scenario: Verbose mode

- **WHEN** `--verbose` or `-v` flag is used
- **THEN** the system SHALL display additional details
- **AND** show timestamps
- **AND** show internal operation logs

### Requirement: CLI Error Handling

The system SHALL provide helpful error messages for CLI commands.

#### Scenario: Missing configuration

- **WHEN** config file doesn't exist
- **THEN** the system SHALL display: "Configuration not found"
- **AND** suggest: "Run 'nanobot onboard' to create configuration"

#### Scenario: Invalid parameter

- **WHEN** user provides invalid parameter
- **THEN** the system SHALL display: "Invalid parameter: <param>"
- **AND** show expected format
- **AND** display command usage

#### Scenario: Operation failure

- **WHEN** an operation fails (e.g., API call, file write)
- **THEN** the system SHALL display error message
- **AND** show error details in verbose mode
- **AND** suggest possible solutions
