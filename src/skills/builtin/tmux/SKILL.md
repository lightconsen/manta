---
name: tmux
description: "Manage tmux sessions, windows, and panes for terminal multiplexing"
version: "1.0.0"
author: "manta"
triggers:
  - type: command
    pattern: "tmux"
    priority: 100
  - type: keyword
    pattern: "terminal session"
    priority: 80
  - type: keyword
    pattern: "tmux session"
    priority: 90
openclaw:
  emoji: "🖥️"
  category: "development"
  tags:
    - "tmux"
    - "terminal"
    - "sessions"
  requires:
    bins: ["tmux"]
---

# tmux Skill

Manage terminal multiplexer sessions for persistent, multi-pane workflows.

## Capabilities

- Create, list, attach, and kill tmux sessions
- Manage windows and panes within sessions
- Run commands in specific panes
- Send keystrokes to tmux panes
- Capture pane output for reading

## Usage Examples

### Create a session
"Create a tmux session called dev" or "Start a new tmux session"

### List sessions
"Show all tmux sessions" or "What tmux sessions are running?"

### Run a command in tmux
"Run 'npm start' in the dev session" or "Execute the build command in tmux"

### Capture pane output
"Show me the output from the server pane" or "What's in the log pane?"

## Common Commands

```bash
# Create named session
tmux new-session -d -s myproject

# List sessions
tmux list-sessions

# Send command to pane
tmux send-keys -t myproject "npm run dev" Enter

# Split window
tmux split-window -h -t myproject

# Capture pane output
tmux capture-pane -t myproject -p
```

## Best Practices

1. Use descriptive session names tied to projects
2. Create layouts (editor, server, logs) using pane splits
3. Use `send-keys` with `Enter` to execute commands
4. Always check if a session exists before creating
