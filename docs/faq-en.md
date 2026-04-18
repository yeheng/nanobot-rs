# Frequently Asked Questions

> Common issues and solutions when using Gasket

---

## 🚀 Installation & Startup

### Q: Build fails with missing dependencies?

**A:** Ensure basic development tools are installed:

```bash
# macOS
xcode-select --install

# Ubuntu/Debian
sudo apt-get install build-essential pkg-config libssl-dev

# Fedora/RHEL
sudo dnf install gcc openssl-devel
```

### Q: Can't find `gasket` command after installation?

**A:** Ensure Cargo bin directory is in PATH:

```bash
# Add to ~/.bashrc or ~/.zshrc
export PATH="$HOME/.cargo/bin:$PATH"

# Apply immediately
source ~/.bashrc  # or source ~/.zshrc
```

### Q: Does it work on Windows?

**A:** Yes! But requires:
1. Install [WSL2](https://docs.microsoft.com/en-us/windows/wsl/install) (recommended)
2. Or install [Rust](https://rustup.rs) and [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) directly on Windows

---

## 🔑 API & Models

### Q: Which model should I use?

**A:** Choose based on your needs:

| Use Case | Recommended Model | Price |
|----------|-------------------|-------|
| General chat | Claude 3.5 Sonnet | Medium |
| Programming | Claude 4.5 Sonnet / DeepSeek Coder | High/Low |
| Fast response | GPT-4o-mini / GLM-4-Flash | Low |
| Chinese tasks | DeepSeek-V3 / Zhipu GLM-5 | Low |
| Reasoning | DeepSeek-R1 / o1 | High |

### Q: How to save on API costs?

**A:**
1. **Use cheaper models**: DeepSeek, Zhipu cost 1/10th of Claude
2. **Limit tokens**: Set `max_tokens` in config
3. **Use local models**: Configure Ollama for local execution (free but slower)

```yaml
agents:
  defaults:
    model: deepseek/deepseek-chat  # Cheaper option
    max_tokens: 2000               # Limit output length
```

### Q: Where to safely store API Key?

**A:** Recommended approaches:

1. **Use Vault encryption** (most secure):
```bash
# Set password
export GASKET_MASTER_PASSWORD="your-strong-password"

# Store API Key
gasket vault set openrouter_api_key sk-or-v1-xxx

# Use placeholder in config
# config.yaml:
# api_key: "{{vault:openrouter_api_key}}"
```

2. **Use environment variables** (secondary):
```yaml
providers:
  openrouter:
    api_key: ${OPENROUTER_API_KEY}
```

### Q: Why is AI response slow?

**A:** Possible causes:
- Model itself is slow (DeepSeek-R1 takes tens of seconds to reason)
- Network issues (accessing OpenAI from China is slow)
- Streaming output being buffered

Solutions:
- Switch to faster models (GPT-4o-mini, GLM-4-Flash)
- Use domestic providers (DeepSeek, Zhipu)
- Check network connection

---

## 💬 Conversation & Memory

### Q: AI forgot what I said earlier?

**A:** This is normal. Gasket keeps last 50 messages by default (configurable). But you can:

1. **Use long-term memory**: Have AI save important info to memory files
2. **Adjust history length**: Modify config

```yaml
agents:
  defaults:
    memory_window: 100  # Increase history window
```

### Q: How to clear conversation history?

**A:** Three ways:

```bash
# Method 1: Use command in interactive mode
You: /new

# Method 2: Interactive command
Type /new in the gasket agent interactive session

# Method 3: Delete database (complete reset)
rm ~/.gasket/gasket.db
```

### Q: How to make AI remember my information?

**A:** Edit `~/.gasket/PROFILE.md`:

```markdown
# User Profile

- Name: Alice
- Profession: Software Engineer
- Tech stack: Rust, Python
- Preference: Concise code style
```

Or ask AI to remember:

```
You: Remember I prefer spaces over tabs
🤖 Gasket: Recorded in your preferences
```

### Q: Where is conversation history stored?

**A:** Default storage locations:

```
~/.gasket/gasket.db  # Conversation history, memory index
~/.gasket/memory/     # Long-term memory files (Markdown)
```

---

## 🛠️ Tool Usage

### Q: Can AI execute dangerous commands?

**A:** Default safety restrictions apply. Configurable:

```yaml
tools:
  exec:
    # Note: exec tool safety policy is managed via policy config
    policy:
      allowlist: ["git", "cargo", "ls", "cat"]
      denylist: ["rm", "sudo"]
    # Or allow_all - allow everything (dangerous!)
```

### Q: How to let AI read my project files?

**A:** AI has file tools, but needs proper guidance:

```
You: Read ./src/main.rs and explain this code
🤖 Gasket: (calls read_file tool)...
```

### Q: Web search not working?

**A:** Need to configure search API:

```yaml
tools:
  web:
    search:
      provider: brave  # or tavily, exa
      api_key: your-key
```

Free search options:
- [Brave Search API](https://brave.com/search/api/) (2000 free queries/month)
- [Serper](https://serper.dev) (2500 free queries/month)

---

## 📱 Multi-Channel Configuration

### Q: How to connect Telegram?

**A:**

1. Search @BotFather on Telegram
2. Send `/newbot` to create a bot
3. Copy the token
4. Configure:

```yaml
channels:
  telegram:
    token: "123456:ABC-DEF..."
    allow_from: []  # Empty allows everyone
```

5. Start: `gasket gateway`

### Q: How to connect Discord?

**A:**

1. Visit [Discord Developer Portal](https://discord.com/developers/applications)
2. Create Application → Bot → Copy Token
3. Configure:

```yaml
channels:
  discord:
    token: "MTAx..."
```

4. Invite bot to server (needs Message Content Intent permission)

### Q: How to restrict specific users?

**A:**

```yaml
channels:
  telegram:
    token: "..."
    allow_from:
      - "123456789"   # Your Telegram ID
      - "987654321"   # Friend's ID
```

Get user ID: Send message to @userinfobot on Telegram

---

## ⚡ Performance & Troubleshooting

### Q: How to view logs?

**A:**

```bash
# Set log level
export RUST_LOG=debug   # debug/info/warn/error

# Run
gasket agent

# Or with systemd
sudo journalctl -u gasket -f
```

### Q: Database corrupted?

**A:** Restore from backup:

```bash
# If backups enabled
cp /backup/gasket/gasket.db ~/.gasket/

# Or use SQLite repair
sqlite3 ~/.gasket/gasket.db ".recover" > recovery.sql
sqlite3 ~/.gasket/gasket.db.new < recovery.sql
```

### Q: How to backup data?

**A:**

```bash
#!/bin/bash
# backup.sh

DATE=$(date +%Y%m%d)
mkdir -p ~/backup/gasket/$DATE

# Backup database
cp ~/.gasket/gasket.db ~/backup/gasket/$DATE/

# Backup memory
tar czf ~/backup/gasket/$DATE/memory.tar.gz ~/.gasket/memory/

# Backup config
cp ~/.gasket/config.yaml ~/backup/gasket/$DATE/

echo "Backup complete: ~/backup/gasket/$DATE"
```

---

## 🔄 Updates & Maintenance

### Q: How to update Gasket?

**A:**

```bash
cd gasket-rs
git pull
cargo build --release
cargo install --path cli
```

### Q: How to completely uninstall?

**A:**

```bash
# Uninstall program
cargo uninstall gasket-cli

# Delete data (careful!)
rm -rf ~/.gasket
```

---

## 💡 Other Questions

### Q: What's the difference between Gasket and other AI clients?

**A:**

| Feature | Gasket | Regular Client |
|---------|--------|----------------|
| Long-term memory | ✅ Filesystem + SQLite | ❌ None |
| Scheduled tasks | ✅ Built-in Cron | ❌ None |
| Multi-channel | ✅ Telegram/Discord/... | Usually single |
| Tool calling | ✅ Rich built-in tools | Limited |
| Subagents | ✅ Parallel tasks | ❌ None |
| Privacy | ✅ Local storage | Usually cloud |

### Q: What scenarios is it suitable for?

**A:**

- ✅ Personal AI assistant (24/7 companion)
- ✅ Programming assistant (code review, generation)
- ✅ Knowledge management (long-term memory, search)
- ✅ Automated tasks (reminders, workflows)
- ✅ Multi-platform bot (Telegram/Discord bot)

### Q: How to contribute code?

**A:** Visit [GitHub](https://github.com/YeHeng/gasket-rs), welcome to:
- Submit Issues to report bugs
- Submit PRs to improve features
- Improve documentation

---

## Still Need Help?

- 📖 Check [Configuration Guide](config-en.md)
- 📖 Check [Deployment Guide](deployment-en.md)
- 🐛 Submit [Issue](https://github.com/YeHeng/gasket-rs/issues)
