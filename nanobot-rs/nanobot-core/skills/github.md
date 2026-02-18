---
name: github
description: GitHub CLI integration for repository management, issues, PRs, and more
always: false
bins:
  - gh
env_vars:
  - GITHUB_TOKEN
---

# GitHub Skill

This skill provides integration with GitHub using the `gh` CLI tool.

## Prerequisites

- GitHub CLI (`gh`) installed and configured
- `GITHUB_TOKEN` environment variable set with appropriate permissions

## Common Operations

### Repository Management

```bash
# List repositories
gh repo list --limit 50

# Create a new repository
gh repo create <name> --public/--private

# Clone a repository
gh repo clone <owner>/<repo>

# View repository info
gh repo view <owner>/<repo>
```

### Issues

```bash
# List issues
gh issue list --state open --limit 20

# Create an issue
gh issue create --title "<title>" --body "<description>"

# View issue details
gh issue view <number>

# Close an issue
gh issue close <number>
```

### Pull Requests

```bash
# List PRs
gh pr list --state open

# Create a PR
gh pr create --title "<title>" --body "<description>"

# Review a PR
gh pr view <number>

# Merge a PR
gh pr merge <number> --squash/--merge

# Check PR status
gh pr status
```

### Workflows

```bash
# List workflows
gh workflow list

# Run a workflow
gh workflow run <workflow-name>

# View workflow runs
gh run list --limit 10
```

### Gists

```bash
# Create a gist
gh gist create <file> --public/--secret

# List your gists
gh gist list
```

## Best Practices

1. Always use `--json` flag for machine-readable output when processing results
2. Use `--jq` for filtering JSON output
3. Set up aliases for frequently used commands

## Example Usage

When asked to help with GitHub operations:

1. First check the current repository context with `gh repo view`
2. Use appropriate commands based on the task
3. Provide clear summaries of actions taken
