# Quick Start Guide

> Get Gasket running in 5 minutes

---

## Goal

After completing this guide, you will:
- ✅ Install and run Gasket
- ✅ Have your first conversation with AI
- ✅ Learn basic commands

---

## Step 1: Install Rust

Gasket is written in Rust. First, install Rust:

```bash
# Mac/Linux
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Verify installation
rustc --version
```

> 💡 **Windows users**: Download the installer from [rustup.rs](https://rustup.rs)

---

## Step 2: Download and Build Gasket

```bash
# Clone the code
git clone https://github.com/YeHeng/gasket-rs.git
cd gasket-rs

# Build (grab a coffee, takes 2-5 minutes)
cargo build --release

# Install to system
cargo install --path cli

# Verify installation
gasket --version
```

```mermaid
flowchart LR
    A[Download] --> B[Build] --> C[Install] --> D[Run]
    
    style B fill:#ffe4b5
    style D fill:#90ee90
```

---

## Step 3: Initialize Workspace

```bash
# Create config and workspace
gasket onboard
```

Example output:
```
🚀 Initializing Gasket workspace...

✓ Created directory: ~/.gasket
✓ Generated config: ~/.gasket/config.yaml
✓ Created profile: ~/.gasket/PROFILE.md
✓ Created memory dir: ~/.gasket/memory/
✓ Created skills dir: ~/.gasket/skills/

Next steps:
1. Edit ~/.gasket/config.yaml to add API Key
2. Run: gasket agent
```

---

## Step 4: Configure API Key

### Get a Free API Key

We recommend [OpenRouter](https://openrouter.ai) (new users get free credits):

1. Visit [openrouter.ai](https://openrouter.ai)
2. Create an account
3. Create an API Key
4. Copy the Key (format: `sk-or-v1-...`)

### Edit Configuration

Open `~/.gasket/config.yaml`:

```yaml
providers:
  openrouter:
    api_key: sk-or-v1-your-key-here  # ← Replace with your key

agents:
  defaults:
    model: openrouter/anthropic/claude-4.5-sonnet
```

> 💡 **Budget tip**: Use `deepseek/deepseek-chat` or `zhipu/glm-4-flash` for lower cost

---

## Step 5: Start Chatting!

```bash
# Launch interactive mode
gasket agent
```

You'll see:
```
🤖 Gasket v2.0.0
Model: openrouter/anthropic/claude-4.5-sonnet

You: Hello!
🤖 Gasket: Hello! Nice to meet you. How can I help you today?

You: Write a Fibonacci function in Python
🤖 Gasket: Here's a simple implementation:

```python
def fibonacci(n):
    if n <= 1:
        return n
    return fibonacci(n-1) + fibonacci(n-2)

# Test
for i in range(10):
    print(f"F({i}) = {fibonacci(i)}")
```

You: /new
🤖 Gasket: Started a new conversation, history cleared.
```

---

## Common Commands

### Interactive Mode Commands

| Command | Action |
|---------|--------|
| `/new` | Start new conversation (clear history) |
| `/help` | Show help |
| `/exit` | Exit |

### CLI Commands

```bash
# Single message (non-interactive)
gasket agent -m "Hello"

# Start fresh
gasket agent --new

# Use specific model
gasket agent --model deepseek/deepseek-chat

# Launch Gateway (multi-channel service)
gasket gateway
```

---

## Next Steps

Congratulations! Gasket is running 🎉

### Continue Exploring

- 📖 Read [Kernel](kernel-en.md) to understand how it works
- 🧠 Read [Memory & History](memory-history-en.md) to understand how AI remembers you
- 🔧 Read [Config Guide](config-en.md) for more configuration options
- ⏰ Read [Cron](cron-en.md) to set up automated tasks

### Add More Channels

```yaml
# config.yaml - Add Telegram
channels:
  telegram:
    token: your-bot-token  # Get from @BotFather
```

Then run `gasket gateway`, and your AI can respond on Telegram!

---

## Troubleshooting

| Issue | Solution |
|-------|----------|
| Build fails | Ensure Rust >= 1.75 (`rustc --version`) |
| API error | Check if API Key is correct and has credits |
| Command not found | Ensure `~/.cargo/bin` is in PATH |

More help in [FAQ](faq-en.md) or [Issues](https://github.com/YeHeng/gasket-rs/issues)
