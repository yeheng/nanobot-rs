# GitHub Copilot Configuration Guide

Gasket supports GitHub Copilot as an LLM provider. We offer two authentication methods: **OAuth Device Flow (Recommended)** and **PAT (Personal Access Token)**.

## Method 1: OAuth Device Flow (Recommended)

Use the OAuth device flow to authenticate with your GitHub account that has a Copilot subscription.

### Steps

1. Run the following command in your terminal:
   ```bash
   gasket auth copilot
   ```
2. The terminal will output a device code and a GitHub verification URL.
3. Copy the device code, open the verification URL in your browser, and paste the code.
4. Authorize the application to access your GitHub account in the browser.
5. After authorization, the terminal will show `Successfully authenticated!` and automatically save the token to `~/.gasket/config.yaml`.

> **Note**: Some GitHub accounts may restrict third-party clients from obtaining Copilot tokens via OAuth. If this method fails, try Method 2 (PAT) below.

---

## Method 2: PAT Authorization (Personal Access Token)

If you cannot use OAuth, or need to use Copilot in a headless environment (e.g., a server), you can generate a Personal Access Token (PAT).

### Steps

1. Log in to GitHub and visit [Personal Access Tokens (Tokens (classic))](https://github.com/settings/tokens).
2. Click **Generate new token (classic)**.
3. Fill in the Note (e.g., `gasket-copilot`), and **make sure to check the `copilot` scope**.
   *(If you don't see the `copilot` option, ensure your account has a GitHub Copilot subscription.)*
4. Click Generate and copy the token (`ghp_...`).
5. Run the following command in your terminal to complete the setup:
   ```bash
   gasket auth copilot --pat <your-pat-token>
   ```

After successful validation, the terminal will show "Token validated successfully" and automatically save the configuration.

---

## Switching to Copilot as the Default Model

Regardless of the authentication method, after successful authentication, you can set Copilot as the default model in two ways:

### 1. Temporary Use (Command-line Argument)
```bash
gasket agent -m copilot/gpt-4o
```

### 2. Permanent Configuration (Edit Config File)
Open `~/.gasket/config.yaml` and set the default model to `copilot/gpt-4o`:
```yaml
agents:
  defaults:
    model: copilot/gpt-4o
```

Now you can use the powerful GitHub Copilot model in CLI, Telegram, or other channels.
