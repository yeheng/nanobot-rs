# Vault Usage Guide

Vault is Gasket's sensitive data isolation module for securely storing and managing sensitive information (API keys, passwords, database connection strings, etc.).

## Table of Contents

- [Overview](#overview)
- [Design Principles](#design-principles)
- [Quick Start](#quick-start)
- [CLI Command Reference](#cli-command-reference)
- [Placeholder Syntax](#placeholder-syntax)
- [Security Best Practices](#security-best-practices)
- [Common Use Cases](#common-use-cases)

## Overview

Vault's core function is to completely isolate sensitive data from LLM-accessible storage, ensuring:

- Sensitive data is never persisted to LLM memory or history
- Sensitive data is only injected at the last moment before sending to LLM
- Supports placeholder syntax `{{vault:key}}` in messages

### Storage Location

```
~/.gasket/vault/secrets.json
```

## Design Principles

### 1. Data Structure Isolation

VaultStore is completely independent from memory and history storage systems, stored in a separate JSON file.

### 2. Runtime Injection

Through the `VaultInjector` interceptor, placeholders are scanned and replaced before messages are sent to LLM:

```rust
// Placeholder in message
"Connect to database with {{vault:db_password}}"

// Replaced at runtime with
"Connect to database with actual_password_value"
```

### 3. Zero-Trust Design

Sensitive data never appears in:
- LLM's persistent storage
- Log files (supports log redaction)
- Session history

## Quick Start

### 1. Set a Secret

```bash
# Interactive setup (prompts for value)
gasket vault set openai_api_key

# Direct value and description
gasket vault set db_password --value "my_secret_password" --description "Database password"
```

### 2. List Secrets

```bash
# List all keys (values hidden)
gasket vault list
```

Example output:

```
Vault Entries

Key                 Description                               Created
────────────────────────────────────────────────────────────────────────────────
openai_api_key      OpenAI API key for chat completions       2024-01-15 10:30
db_password         Database password                         2024-01-15 11:00

Total: 2 entries

Tip: Use {{vault:key}} in your messages to inject secrets at runtime.
```

### 3. Use in Conversation

When chatting with the Agent, use placeholder syntax:

```
Please use API key {{vault:openai_api_key}} to call OpenAI API
```

In the message received by the Agent, `{{vault:openai_api_key}}` is automatically replaced with the actual key value.

## CLI Command Reference

### `gasket vault list`

List all vault entries (values hidden).

```bash
gasket vault list
```

### `gasket vault set`

Set a vault entry.

```bash
# Interactive setup
gasket vault set <key>

# Direct setup
gasket vault set <key> --value <value> --description <description>

# Shorthand
gasket vault set <key> -v <value> -d <description>
```

**Parameters:**
- `key` - Key name (letters, numbers, and underscores only)
- `--value, -v` - Key value (optional, prompts interactively if not provided)
- `--description, -d` - Description (optional)

**Examples:**

```bash
# Interactive setup (password input hidden)
gasket vault set api_key

# Direct setup
gasket vault set aws_access_key -v "AKIAIOSFODNN7EXAMPLE" -d "AWS Access Key"

# Update existing entry (preserves creation time)
gasket vault set api_key -v "new_value"
```

### `gasket vault get`

Get key value (outputs directly to stdout).

```bash
gasket vault get <key>
```

**Note:** This command outputs the secret value directly, use with caution.

**Use Cases:**

```bash
# Use in scripts
export API_KEY=$(gasket vault get openai_api_key)

# Copy to clipboard (macOS)
gasket vault get api_key | pbcopy
```

### `gasket vault show`

Show detailed information about a key.

```bash
gasket vault show <key>

# Show key value
gasket vault show <key> --show-value
```

**Parameters:**
- `--show-value` - Show key value (hidden by default)

**Example Output:**

```
openai_api_key

  Description: OpenAI API key
  Created:     2024-01-15 10:30 UTC
  Last used:   2024-01-16 14:22 UTC

  Use --show-value to display the secret value.

  Usage: {{vault:openai_api_key}}
```

### `gasket vault delete`

Delete a key.

```bash
gasket vault delete <key>

# Skip confirmation prompt
gasket vault delete <key> --force
```

**Parameters:**
- `--force, -f` - Skip confirmation prompt

### `gasket vault import`

Import keys from JSON file.

```bash
gasket vault import <file>

# Merge mode (don't overwrite existing keys)
gasket vault import <file> --merge
```

**Parameters:**
- `--merge, -m` - Merge mode, don't overwrite existing keys

**JSON File Format:**

```json
{
  "api_key": {
    "key": "api_key",
    "value": "sk-xxxx",
    "description": "API Key",
    "created_at": "2024-01-15T10:30:00Z",
    "last_used": null
  }
}
```

### `gasket vault export`

Export all keys to JSON file.

```bash
gasket vault export <file>
```

**Example:**

```bash
gasket vault export ~/backup/vault_backup.json
```

**Note:** Exported file contains all key values, keep it secure!

## Placeholder Syntax

Use placeholders in messages with `{{vault:key_name}}` format:

### Basic Usage

```
Use {{vault:api_key}} to access API
```

### Multiple Placeholders

```
Connect to database:
- Host: db.example.com
- User: admin
- Password: {{vault:db_password}}
```

### Supported Locations

Placeholders can appear anywhere in messages:

```
System prompt: You are an assistant, authenticate with {{vault:system_token}}.
User message: Please help me call {{vault:service_name}} service.
```

### Placeholder Naming Rules

Key names must:
- Be non-empty
- Contain only letters (a-z, A-Z), numbers (0-9), and underscores (_)
- Start with letter or underscore

**Valid Examples:**
- `api_key`
- `db_password_v2`
- `AWS_SECRET_KEY`
- `token123`

**Invalid Examples:**
- `api-key` (contains hyphen)
- `db.password` (contains dot)
- `123key` (starts with number, technically allowed but not recommended)

## Security Best Practices

### 1. File Permissions

Ensure correct vault file permissions:

```bash
chmod 600 ~/.gasket/vault/secrets.json
```

### 2. Don't Hardcode in Code

Avoid:

```rust
// ❌ Don't do this
let api_key = "sk-xxxx";
```

Recommend:

```rust
// ✅ Use vault
let message = "Use {{vault:api_key}} to authenticate";
```

### 3. Regular Key Rotation

```bash
# Update key value
gasket vault set api_key -v "new_secret_value"
```

### 4. Backup and Restore

```bash
# Backup
gasket vault export ~/secure_backup/vault_$(date +%Y%m%d).json

# Restore
gasket vault import ~/secure_backup/vault_20240115.json
```

### 5. Audit Usage

```bash
# View last used time
gasket vault show api_key
```

### 6. Principle of Least Privilege

Only set access for keys that are needed, delete unused keys:

```bash
gasket vault delete unused_key
```

## Common Use Cases

### API Key Management

```bash
# Set multiple API keys
gasket vault set openai_api_key -d "OpenAI API Key"
gasket vault set anthropic_api_key -d "Anthropic API Key"
gasket vault set github_token -d "GitHub Personal Access Token"
```

In conversation:

```
Please use {{vault:openai_api_key}} to call GPT-4 API to help me analyze this code.
```

### Database Connection

```bash
# Store database credentials
gasket vault set db_host -v "db.example.com" -d "Database host"
gasket vault set db_user -v "admin" -d "Database user"
gasket vault set db_password -v "secret123" -d "Database password"
```

In conversation:

```
Help me write a database query to connect to {{vault:db_host}}, username is admin, password is {{vault:db_password}}
```

### AWS Credentials

```bash
gasket vault set aws_access_key -v "AKIAIOSFODNN7EXAMPLE" -d "AWS Access Key"
gasket vault set aws_secret_key -v "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY" -d "AWS Secret Key"
gasket vault set aws_region -v "us-east-1" -d "AWS Region"
```

### CI/CD Integration

Use in CI/CD pipelines:

```yaml
# .github/workflows/deploy.yml
- name: Get API Key
  run: |
    export API_KEY=$(gasket vault get production_api_key)
    ./deploy.sh
```

## Troubleshooting

### Key Not Found

If you see `Key not found` error:

1. Check key name spelling
2. Use `gasket vault list` to view all available keys
3. Ensure key name contains only letters, numbers, and underscores

### Placeholder Not Replaced

If placeholders in messages are not replaced:

1. Confirm correct format: `{{vault:key}}` (note double braces)
2. Check if key exists
3. Check if key name is correct

### Permission Errors

If you encounter file permission errors:

```bash
# Fix permissions
chmod 700 ~/.gasket/vault
chmod 600 ~/.gasket/vault/secrets.json
```

## Related Documentation

- [Architecture Overview](./architecture.md)
- [Data Structures](./data-structures.md)
- [Modules](./modules.md)
