# 快速开始指南

> 5 分钟让 Gasket 跑起来

---

## 目标

完成本指南后，你将：
- ✅ 安装并运行 Gasket
- ✅ 与 AI 进行第一次对话
- ✅ 了解基本命令

---

## 第一步：安装 Rust

Gasket 是用 Rust 编写的，先安装 Rust：

```bash
# Mac/Linux
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# 验证安装
rustc --version
```

> 💡 **Windows 用户**: 从 [rustup.rs](https://rustup.rs) 下载安装程序

---

## 第二步：下载并编译 Gasket

```bash
# 克隆代码
git clone https://github.com/YeHeng/gasket.git
cd gasket-rs

# 编译（喝杯咖啡，大约需要 2-5 分钟）
cargo build --release

# 安装到系统
cargo install --path cli

# 验证安装
gasket --version
```

```mermaid
flowchart LR
    A[下载代码] --> B[编译] --> C[安装] --> D[运行]
    
    style B fill:#ffe4b5
    style D fill:#90ee90
```

---

## 第三步：初始化工作空间

```bash
# 创建配置和工作空间
gasket onboard
```

输出示例：
```
🚀 初始化 Gasket 工作空间...

✓ 创建目录: ~/.gasket
✓ 生成配置: ~/.gasket/config.yaml
✓ 创建个人资料: ~/.gasket/PROFILE.md
✓ 创建记忆目录: ~/.gasket/memory/
✓ 创建技能目录: ~/.gasket/skills/

下一步:
1. 编辑 ~/.gasket/config.yaml 填入 API Key
2. 运行: gasket agent
```

---

## 第四步：配置 API Key

### 获取免费 API Key

推荐 [OpenRouter](https://openrouter.ai)（新用户有免费额度）：

1. 访问 [openrouter.ai](https://openrouter.ai)
2. 注册账号
3. 创建 API Key
4. 复制 Key（格式如 `sk-or-v1-...`）

### 编辑配置

打开 `~/.gasket/config.yaml`：

```yaml
providers:
  openrouter:
    api_key: sk-or-v1-your-key-here  # ← 替换为你的 Key

agents:
  defaults:
    model: openrouter/anthropic/claude-4.5-sonnet
```

> 💡 **省钱提示**: 可以换成 `deepseek/deepseek-chat` 或 `zhipu/glm-4-flash`，价格更低

---

## 第五步：开始对话！

```bash
# 启动交互模式
gasket agent
```

你会看到：
```
🤖 Gasket v2.0.0
Model: openrouter/anthropic/claude-4.5-sonnet

你: 你好！
🤖 Gasket: 你好！很高兴见到你，有什么我可以帮助你的吗？

你: 用 Python 写一个斐波那契数列函数
🤖 Gasket: 当然可以！这是一个简单的实现：

```python
def fibonacci(n):
    if n <= 1:
        return n
    return fibonacci(n-1) + fibonacci(n-2)

# 测试
for i in range(10):
    print(f"F({i}) = {fibonacci(i)}")
```

你: /new
🤖 Gasket: 已开启新对话，历史已清空。
```

---

## 常用命令

### 交互模式命令

| 命令 | 作用 |
|------|------|
| `/new` | 开启新对话（清空历史） |
| `/help` | 显示帮助 |
| `/exit` | 退出 |

### CLI 命令

```bash
# 单次对话（非交互）
gasket agent -m "你好"

# 开启新对话
gasket agent --new

# 使用指定模型
gasket agent --model deepseek/deepseek-chat

# 启动 Gateway（多渠道服务）
gasket gateway
```

---

## 下一步

恭喜你！Gasket 已经跑起来了 🎉

### 继续探索

- 📖 阅读 [AI 大脑核心](kernel.md) 了解工作原理
- 🧠 阅读 [记忆与历史](memory-history.md) 理解 AI 如何记住你
- 🔧 阅读 [配置指南](config.md) 了解更多配置选项
- ⏰ 阅读 [定时任务](cron.md) 设置自动提醒

### 加入更多渠道

```yaml
# config.yaml - 添加 Telegram
channels:
  telegram:
    token: your-bot-token  # 从 @BotFather 获取
```

然后运行 `gasket gateway`，你的 AI 就能在 Telegram 上回复你了！

---

## 遇到问题？

| 问题 | 解决 |
|------|------|
| 编译失败 | 确保 Rust 版本 >= 1.75 (`rustc --version`) |
| API 错误 | 检查 API Key 是否正确，是否还有额度 |
| 找不到命令 | 确保 `~/.cargo/bin` 在 PATH 中 |

更多帮助查看 [FAQ](faq.md) 或提交 [Issue](https://github.com/YeHeng/gasket-rs/issues)
