# GitHub Copilot Configuration Guide

> How to use GitHub Copilot as an LLM provider in Gasket

---

## Overview

GitHub Copilot can be used as an LLM provider in Gasket through two authentication methods:

1. **PAT (Personal Access Token)** - Easier setup, but rate limited
2. **OAuth** - More complex setup, higher rate limits

---

## Method 1: Personal Access Token (PAT)

### Step 1: Get GitHub Token

1. Visit [GitHub Settings → Developer settings → Personal access tokens](https://github.com/settings/tokens)
2. Click "Generate new token (classic)"
3. Select scopes: `read:user`, `repo`
4. Generate and copy the token

### Step 2: Configure Gasket

```yaml
providers:
  copilot:
    pat: ghp_your_token_here
```

### Limitations

- Lower rate limits
- May be subject to Copilot's usage policies
- Not recommended for production use

---

## Method 2: OAuth Authentication

### Step 1: Register OAuth App

1. Visit [GitHub Settings → Developer settings → OAuth Apps](https://github.com/settings/developers)
2. Click "New OAuth App"
3. Fill in:
   - Application name: `Gasket`
   - Homepage URL: `http://localhost`
   - Authorization callback URL: `http://localhost:8080/callback`
4. Save and note the Client ID and Client Secret

### Step 2: Get OAuth Token

Run Gasket's OAuth flow:

```bash
gasket copilot auth
```

This will:
1. Open browser for GitHub authorization
2. Save the OAuth token to `~/.gasket/copilot-token.json`

### Step 3: Configure Gasket

```yaml
providers:
  copilot:
    oauth_token_path: ~/.gasket/copilot-token.json
```

---

## Using Copilot in Gasket

### Configuration

```yaml
providers:
  copilot:
    pat: ${COPILOT_PAT}  # or oauth_token_path

agents:
  defaults:
    model: copilot/copilot-chat
```

### Available Models

| Model | Description |
|-------|-------------|
| `copilot/copilot-chat` | Standard Copilot chat model |
| `copilot/copilot-chat-prompt` | Optimized for prompt completion |

---

## Troubleshooting

### "Authentication failed"

- Check if token is expired
- For OAuth, re-run `gasket copilot auth`
- Verify token has required scopes

### "Rate limit exceeded"

- Switch to OAuth method for higher limits
- Add rate limiting in your application
- Consider using other providers as fallback

### "Model not available"

- Ensure your GitHub account has Copilot subscription
- Check if Copilot is enabled in your account
