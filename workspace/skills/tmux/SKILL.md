---
name: tmux
description: Terminal multiplexer session management
always: false
bins:
  - tmux
---

# Tmux

## Sessions

```bash
tmux new-session -d -s <name>          # create detached
tmux attach -t <name>                  # attach
tmux ls                                # list
tmux kill-session -t <name>            # kill
tmux kill-server                       # kill all
```

## Windows & Panes

```bash
tmux new-window -t <s> -n <name>       # new window
tmux split-window -h/-v -t <s>         # split pane
tmux select-window -t <s>:<idx>        # switch window
```

## Commands

```bash
tmux send-keys -t <s> "cmd" Enter      # send command
tmux send-keys -t <s> C-c              # Ctrl+C
tmux capture-pane -t <s> -p            # capture output
tmux capture-pane -t <s> -p -S -100    # last 100 lines
```

## Patterns

**Background task:**
```bash
tmux new-session -d -s bg
tmux send-keys -t bg "cd /p && npm run build" Enter
tmux capture-pane -t bg -p
```
