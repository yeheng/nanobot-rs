---
name: tmux
description: Terminal multiplexer session management for persistent terminals
always: false
bins:
  - tmux
---

# Tmux Skill

This skill provides tmux terminal multiplexer management capabilities for creating and managing persistent terminal sessions.

## Session Management

### Creating Sessions

```bash
# Create a new named session
tmux new-session -d -s <session-name>

# Create session with a specific command
tmux new-session -d -s <session-name> "<command>"

# Create session with custom window name
tmux new-session -d -s <session-name> -n <window-name>
```

### Listing Sessions

```bash
# List all sessions
tmux list-sessions
tmux ls

# Check if session exists
tmux has-session -t <session-name>
```

### Attaching to Sessions

```bash
# Attach to a session
tmux attach -t <session-name>

# Attach to last session
tmux attach
```

### Killing Sessions

```bash
# Kill a specific session
tmux kill-session -t <session-name>

# Kill all sessions except current
tmux kill-session -a

# Kill all sessions
tmux kill-server
```

## Window Management

```bash
# Create new window
tmux new-window -t <session-name> -n <window-name>

# List windows
tmux list-windows -t <session-name>

# Kill window
tmux kill-window -t <session-name>:<window-index>

# Select window
tmux select-window -t <session-name>:<window-index>
```

## Pane Management

```bash
# Split pane horizontally
tmux split-window -h -t <session-name>

# Split pane vertically
tmux split-window -v -t <session-name>

# List panes
tmux list-panes -t <session-name>

# Send command to pane
tmux send-keys -t <session-name> "<command>" Enter
```

## Sending Commands

```bash
# Send command to a session
tmux send-keys -t <session-name> "echo Hello" Enter

# Send command to specific window
tmux send-keys -t <session-name>:0 "ls -la" Enter

# Send command to specific pane
tmux send-keys -t <session-name>:0.0 "pwd" Enter

# Send Ctrl+C
tmux send-keys -t <session-name> C-c
```

## Capture Output

```bash
# Capture pane content
tmux capture-pane -t <session-name> -p

# Capture with line limit
tmux capture-pane -t <session-name> -p -S -100
```

## Common Patterns

### Background Task Runner

```bash
# Create session for background tasks
tmux new-session -d -s background

# Run a long task
tmux send-keys -t background "cd /project && npm run build" Enter

# Check status later
tmux capture-pane -t background -p
```

### Development Environment

```bash
# Create dev session with multiple windows
tmux new-session -d -s dev -n editor
tmux new-window -t dev -n server
tmux new-window -t dev -n logs

# Setup windows
tmux send-keys -t dev:editor "vim ." Enter
tmux send-keys -t dev:server "npm run dev" Enter
tmux send-keys -t dev:logs "tail -f logs/app.log" Enter
```

## Best Practices

1. Use descriptive session names
2. Kill unused sessions to free resources
3. Use `-d` flag to create detached sessions
4. Always check if session exists before creating
5. Use `capture-pane` for monitoring without attaching

## Example Usage

When asked to manage terminal sessions:

1. "Create a session for running tests" → Create session and run tests
2. "Check the build status" → Capture pane output
3. "Stop the server in background" → Send Ctrl+C to the session
