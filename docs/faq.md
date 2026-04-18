# 常见问题 FAQ

> 使用 Gasket 时可能遇到的问题

---

## 🚀 安装与启动

### Q: 编译失败，提示缺少依赖？

**A:** 确保安装了基础开发工具：

```bash
# macOS
xcode-select --install

# Ubuntu/Debian
sudo apt-get install build-essential pkg-config libssl-dev

# Fedora/RHEL
sudo dnf install gcc openssl-devel
```

### Q: 安装后找不到 `gasket` 命令？

**A:** 确保 Cargo bin 目录在 PATH 中：

```bash
# 添加到 ~/.bashrc 或 ~/.zshrc
export PATH="$HOME/.cargo/bin:$PATH"

# 立即生效
source ~/.bashrc  # 或 source ~/.zshrc
```

### Q: Windows 上能运行吗？

**A:** 可以！但需要：
1. 安装 [WSL2](https://docs.microsoft.com/zh-cn/windows/wsl/install)（推荐）
2. 或在 Windows 上直接安装 [Rust](https://rustup.rs) 和 [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)

---

## 🔑 API 与模型

### Q: 用什么模型比较好？

**A:** 根据需求选择：

| 用途 | 推荐模型 | 价格 |
|------|---------|------|
| 通用对话 | Claude 3.5 Sonnet | 中等 |
| 编程 | Claude 4.5 Sonnet / DeepSeek Coder | 中高/低 |
| 快速响应 | GPT-4o-mini / GLM-4-Flash | 低 |
| 中文场景 | DeepSeek-V3 / 智谱 GLM-5 | 低 |
| 推理任务 | DeepSeek-R1 / o1 | 高 |

### Q: 如何节约 API 费用？

**A:** 
1. **使用便宜的模型**：DeepSeek、智谱的价格是 Claude 的 1/10
2. **限制 token 数**：在配置中设置 `max_tokens`
3. **使用本地模型**：配置 Ollama 运行本地模型（免费但较慢）

```yaml
agents:
  defaults:
    model: deepseek/deepseek-chat  # 便宜好用
    max_tokens: 2000               # 限制输出长度
```

### Q: API Key 放在哪里安全？

**A:** 推荐方案：

1. **使用 Vault 加密**（最安全）：
```bash
# 设置密码
export GASKET_VAULT_PASSWORD="your-strong-password"

# 存储 API Key
gasket vault set openrouter_api_key sk-or-v1-xxx

# 配置文件中使用占位符
# config.yaml:
# api_key: "{{vault:openrouter_api_key}}"
```

2. **使用环境变量**（次安全）：
```yaml
providers:
  openrouter:
    api_key: ${OPENROUTER_API_KEY}
```

### Q: 为什么 AI 回复很慢？

**A:** 可能原因：
- 模型本身较慢（DeepSeek-R1 推理需要几十秒）
- 网络问题（国内访问 OpenAI 较慢）
- 使用了流式输出但被缓存

解决方案：
- 切换到更快的模型（GPT-4o-mini、GLM-4-Flash）
- 使用国内服务商（DeepSeek、智谱）
- 检查网络连接

---

## 💬 对话与记忆

### Q: AI 忘记了之前说的话？

**A:** 这是正常的。Gasket 默认只保留最近 50 条对话（可配置）。但你可以：

1. **使用长期记忆**：让 AI 记住重要信息到记忆文件
2. **调整历史长度**：修改配置

```yaml
agents:
  defaults:
    memory_window: 100  # 增加历史窗口
```

### Q: 如何清空对话历史？

**A:** 三种方式：

```bash
# 方式 1: 交互模式中使用命令
You: /new

# 方式 2: 交互式命令
在 `gasket agent` 交互模式中输入 `/new`

# 方式 3: 删除数据库（彻底清空）
rm ~/.gasket/gasket.db
```

### Q: 如何让 AI 记住我的信息？

**A:** 编辑 `~/.gasket/PROFILE.md`：

```markdown
# 用户资料

- 名字：张三
- 职业：软件工程师
- 技术栈：Rust、Python
- 喜好：简洁的代码风格
```

或者让 AI 帮你记录：

```
你: 记住我喜欢用空格缩进，不用 Tab
🤖 Gasket: 已记录到您的偏好设置中
```

### Q: 对话历史保存在哪里？

**A:** 默认保存在 SQLite 数据库中：

```
~/.gasket/gasket.db  # 会话历史、记忆索引
~/.gasket/memory/     # 长期记忆文件（Markdown）
```

---

## 🛠️ 工具使用

### Q: AI 能执行危险命令吗？

**A:** 默认有安全限制。可在配置中设置：

```yaml
tools:
  exec:
    # 注意：exec 工具的安全策略通过 sandbox 配置管理，
    # 具体参见 docs/tools.md 或 docs/tools-en.md
    allowed_commands:
      - git
      - cargo
      - ls
      - cat
    # 或 deny_list - 只禁止特定命令
    # 或 allow_all - 允许所有（危险！）
```

### Q: 如何让 AI 读取我的项目文件？

**A:** AI 有文件工具，但需要正确引导：

```
你: 读取 ./src/main.rs 并解释这段代码
🤖 Gasket: （调用 read_file 工具）...
```

### Q: 网页搜索不工作？

**A:** 需要配置搜索 API：

```yaml
tools:
  web:
    search:
      provider: brave  # 或 tavily、exa
      api_key: your-key
```

免费的搜索方案：
- [Brave Search API](https://brave.com/search/api/)（每月 2000 次免费）
- [Serper](https://serper.dev)（每月 2500 次免费）

---

## 📱 多渠道配置

### Q: 如何接入 Telegram？

**A:**

1. 在 Telegram 搜索 @BotFather
2. 发送 `/newbot` 创建 Bot
3. 复制 Token
4. 配置：

```yaml
channels:
  telegram:
    token: "123456:ABC-DEF..."
    allow_from: []  # 留空允许所有人
```

5. 启动：`gasket gateway`

### Q: 如何接入 Discord？

**A:**

1. 访问 [Discord Developer Portal](https://discord.com/developers/applications)
2. 创建 Application → Bot → Copy Token
3. 配置：

```yaml
channels:
  discord:
    token: "MTAx..."
```

4. 邀请 Bot 加入服务器（需要 Message Content Intent 权限）

### Q: 如何限制特定用户访问？

**A:**

```yaml
channels:
  telegram:
    token: "..."
    allow_from:
      - "123456789"   # 你的 Telegram ID
      - "987654321"   # 朋友的 ID
```

获取用户 ID：在 Telegram 发送消息给 @userinfobot

---

## ⚡ 性能与故障排查

### Q: 如何查看日志？

**A:**

```bash
# 设置日志级别
export RUST_LOG=debug   # debug/info/warn/error

# 运行
gasket agent

# 或使用 systemd
sudo journalctl -u gasket -f
```

### Q: 数据库损坏怎么办？

**A:** 备份恢复：

```bash
# 如果开启了备份
cp /backup/gasket/gasket.db ~/.gasket/

# 或使用 SQLite 修复
sqlite3 ~/.gasket/gasket.db ".recover" > recovery.sql
sqlite3 ~/.gasket/gasket.db.new < recovery.sql
```

### Q: 如何备份数据？

**A:**

```bash
#!/bin/bash
# backup.sh

DATE=$(date +%Y%m%d)
mkdir -p ~/backup/gasket/$DATE

# 备份数据库
cp ~/.gasket/gasket.db ~/backup/gasket/$DATE/

# 备份记忆
tar czf ~/backup/gasket/$DATE/memory.tar.gz ~/.gasket/memory/

# 备份配置
cp ~/.gasket/config.yaml ~/backup/gasket/$DATE/

echo "备份完成: ~/backup/gasket/$DATE"
```

### Q: Gateway 模式下连接断开？

**A:** 检查：

1. **防火墙**：确保端口开放
2. **超时设置**：增加超时时间
3. **网络稳定性**：使用稳定的网络

```yaml
gateway:
  session_timeout: 7200  # 2小时
```

---

## 🔄 更新与维护

### Q: 如何更新 Gasket？

**A:**

```bash
cd gasket-rs
git pull
cargo build --release
cargo install --path cli
```

### Q: 如何完全卸载？

**A:**

```bash
# 卸载程序
cargo uninstall gasket-cli

# 删除数据（谨慎！）
rm -rf ~/.gasket
```

---

## 💡 其他问题

### Q: Gasket 和其他 AI 客户端有什么区别？

**A:** 

| 特性 | Gasket | 普通客户端 |
|------|--------|-----------|
| 长期记忆 | ✅ 文件系统 + SQLite | ❌ 无 |
| 定时任务 | ✅ 内置 Cron | ❌ 无 |
| 多渠道 | ✅ Telegram/Discord/... | 通常单一 |
| 工具调用 | ✅ 丰富内置工具 | 有限 |
| 子代理 | ✅ 并行任务 | ❌ 无 |
| 隐私 | ✅ 本地存储 | 通常云端 |

### Q: 适合什么场景？

**A:**

- ✅ 个人 AI 助手（全天候陪伴）
- ✅ 编程助手（代码审查、生成）
- ✅ 知识管理（长期记忆、搜索）
- ✅ 自动化任务（定时提醒、工作流）
- ✅ 多平台机器人（Telegram/Discord  bot）

### Q: 如何贡献代码？

**A:** 查看 [GitHub](https://github.com/YeHeng/gasket-rs)，欢迎：
- 提交 Issue 报告 bug
- 提交 PR 改进功能
- 完善文档

---

## 还没解决？

- 📖 查看 [配置指南](config.md)
- 📖 查看 [部署指南](deployment.md)
- 🐛 提交 [Issue](https://github.com/YeHeng/gasket-rs/issues)
