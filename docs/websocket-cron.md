# WebSocket Channel for Cron Jobs

## Overview

WebSocket channel is **enabled by default** in Gasket gateway. When you create a cron job with `channel: websocket`, the scheduled message will be sent to the specified user via WebSocket connection.

## WebSocket Server Setup

The WebSocket server starts automatically when you run:

```bash
gasket gateway
```

**Default Configuration:**
- **URL:** `ws://localhost:3000/ws`
- **Port:** 3000
- **Max Connections:** 1000
- **Backpressure:** 100 messages per connection

## Connecting to WebSocket

### Basic Connection

```javascript
const ws = new WebSocket('ws://localhost:3000/ws');

ws.onopen = () => {
  console.log('Connected to Gasket gateway');
};

ws.onmessage = (event) => {
  console.log('Received:', event.data);
};

ws.onerror = (error) => {
  console.error('WebSocket error:', error);
};
```

### With Authentication

```javascript
const ws = new WebSocket('ws://localhost:3000/ws?token=YOUR_AUTH_TOKEN&user_id=my-user-id');

ws.onmessage = (event) => {
  const message = JSON.parse(event.data);
  console.log('Message from channel:', message.channel);
  console.log('Content:', message.content);
};
```

### Query Parameters

| Parameter | Required | Description |
|-----------|----------|-------------|
| `token` | No | Authentication token (if configured) |
| `user_id` | No | User identifier (defaults to connection ID) |

## Creating Cron Jobs for WebSocket

### Via File (Recommended)

Create a markdown file in `~/.gasket/cron/`:

```markdown
---
name: morning-reminder
cron: "0 0 9 * * *"
channel: websocket
to: alice
enabled: true
---

早上好！今天的工作计划是什么？
```

### Via Agent

Ask the agent to create a cron job:

```
请创建一个 cron 任务，每天早上 9 点通过 websocket 给用户 alice 发送问候
```

The agent will use the `cron` tool with:

```json
{
  "action": "add",
  "name": "Morning Greeting",
  "cron": "0 0 9 * * *",
  "message": "早上好！今天的工作计划是什么？",
  "channel": "websocket",
  "to": "alice"
}
```

### Via CLI

```bash
gasket cron add "Morning Greeting" "0 0 9 * * *" "早上好！" --channel websocket --to alice
```

## Broadcasting to All Users

Omit the `to` field to broadcast to all connected WebSocket clients:

```markdown
---
name: system-maintenance
cron: "0 0 12 * * *"
channel: websocket
enabled: true
---

系统将在 30 分钟后进行维护，请保存您的工作。
```

## Example Use Cases

### 1. Periodic Reminders

```markdown
---
name: water-break
cron: "0 0 * * * *"
channel: websocket
to: developer-1
enabled: true
---

💧 喝水时间到了！站起来活动一下吧。
```

### 2. Scheduled Reports

```markdown
---
name: daily-summary
cron: "0 0 18 * * *"
channel: websocket
to: manager
enabled: true
---

请生成今日团队工作总结报告
```

### 3. System Monitoring Alerts

```markdown
---
name: disk-space-check
cron: "0 0 */6 * * * *"
channel: websocket
to: admin
enabled: true
tool: shell
tool_args:
  command: "df -h / | tail -1 | awk '{print $5}'
---
```

### 4. Heartbeat Notifications

```markdown
---
name: heartbeat
cron: "0 */5 * * * *"
channel: websocket
enabled: true
---

系统运行正常
```

## WebSocket Message Format

Messages received via WebSocket follow this format:

```json
{
  "channel": "websocket",
  "sender_id": "cron",
  "chat_id": "alice",
  "content": "早上好！今天的工作计划是什么？",
  "timestamp": "2026-04-14T09:00:00Z"
}
```

## Connection Management

### Reconnection Strategy

WebSocket connections should implement automatic reconnection:

```javascript
function connect() {
  const ws = new WebSocket('ws://localhost:3000/ws?user_id=alice');
  
  ws.onclose = () => {
    console.log('Connection closed, reconnecting...');
    setTimeout(connect, 5000); // Reconnect after 5 seconds
  };
  
  ws.onerror = (error) => {
    console.error('WebSocket error:', error);
  };
}

connect();
```

### Multiple Users

Each user should connect with a unique `user_id`:

```javascript
// User 1
const ws1 = new WebSocket('ws://localhost:3000/ws?user_id=alice');

// User 2
const ws2 = new WebSocket('ws://localhost:3000/ws?user_id=bob');
```

## Troubleshooting

### Cron Job Not Executing

1. Check if the cron expression is valid:
   ```bash
   gasket cron show <job-id>
   ```

2. Verify the job is enabled:
   ```bash
   gasket cron list
   ```

3. Check gateway logs:
   ```bash
   RUST_LOG=debug gasket gateway
   ```

### WebSocket Not Receiving Messages

1. Verify connection:
   ```javascript
   ws.readyState === 1 // Should be 1 (OPEN)
   ```

2. Check user_id matches the `to` field in cron job

3. Ensure gateway is running with WebSocket support:
   ```bash
   gasket gateway
   # Look for: "WebSocket server listening on 0.0.0.0:3000"
   ```

### Channel Not Found Errors

Make sure the channel name is exactly `websocket` (lowercase):

```yaml
channel: websocket  # ✓ Correct
channel: WebSocket  # ✗ Wrong
```

## Related Documentation

- [Cron Usage Guide](cron-usage.md) - Complete cron job reference
- [Architecture](architecture.md) - System overview
- [Channels Configuration](config.md) - Channel setup
