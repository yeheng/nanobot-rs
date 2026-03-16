# GitHub Copilot 配置指南

Gasket-RS 支持通过 GitHub Copilot 作为 LLM 提供商。我们提供了两种调用方式：**OAuth 设备授权（推荐）** 和 **PAT (Personal Access Token) 授权**。

## 方式一：OAuth 设备授权 (Device Flow)

通过 OAuth 设备授权流，你可以直接使用已开通 Copilot 订阅的 GitHub 账号登录。

### 操作步骤

1. 在终端运行以下命令：
   ```bash
   gasket auth copilot
   ```
2. 终端会输出一串设备码 (Device Code) 和一个 GitHub 验证网址。
3. 复制设备码，在浏览器中打开验证网址，并粘贴该设备码。
4. 在浏览器中授权应用访问你的 GitHub 账号。
5. 授权完成后，终端会提示 `Successfully authenticated!` 并自动将获取到的 Token 保存至 `~/.gasket/config.yaml` 配置文件中。

> **注意**：部分 GitHub 账号可能限制第三方客户端通过 OAuth 获取 Copilot Token。如果此方式失败，请尝试下方的方式二 (PAT 授权)。

---

## 方式二：PAT 授权 (Personal Access Token)

如果你无法使用 OAuth 授权，或者希望在服务器等无浏览器环境中使用，你可以通过生成个人访问令牌 (PAT) 进行授权。

### 操作步骤

1. 登录 GitHub 并访问 [Personal Access Tokens (Tokens (classic))](https://github.com/settings/tokens)。
2. 点击 **Generate new token (classic)**。
3. 填写 Note (如 `gasket-copilot`)，并**必须勾选 `copilot` 权限范围**。
   *(如果列表中没有 `copilot` 选项，请确保你的账号已订阅 GitHub Copilot)*
4. 点击 Generate 并复制生成的令牌 (`ghp_...`)。
5. 在终端运行以下命令完成配置：
   ```bash
   gasket auth copilot --pat <你的PAT令牌>
   ```

成功验证后，终端会提示 Token validated successfully，并自动保存配置。

---

## 修改默认模型使用 Copilot

无论使用哪种授权方式，认证成功后，你可以通过以下两种方式将 Copilot 设为默认模型：

### 1. 临时使用 (命令行参数)
```bash
gasket agent -m copilot/gpt-4o
```

### 2. 永久配置 (修改配置文件)
打开 `~/.gasket/config.yaml`，将默认模型修改为 `copilot/gpt-4o`：
```yaml
agents:
  defaults:
    model: copilot/gpt-4o
```

这样你就可以在 CLI、Telegram 或其他渠道中使用 GitHub Copilot 提供的强大模型了。
