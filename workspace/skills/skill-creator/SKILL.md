---
name: skill-creator
description: Create new gasket skills with proper format
always: false
# bins:        # Only list if the skill truly requires external binaries
#   - <binary>
# env_vars:     # Only list if the skill truly requires environment variables
#   - <ENV_VAR>
---

# Skill Creator

Skills are self-contained Markdown files with YAML frontmatter in `workspace/skills/<name>/SKILL.md`.

## Frontmatter

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Lowercase with hyphens (e.g. `docker-ops`) |
| `description` | Yes | < 100 chars |
| `always` | No | `true` to always load full content (default: `false`) |
| `bins` | No | Required binary commands |
| `env_vars` | No | Required environment variables |

## Template

```markdown
---
name: <skill-name>
description: <brief description>
always: false
# Only include bins/env_vars if you know they are actually required
---

# <Title>

## Prerequisites

## Common Operations

### <Category>

```bash
# commands
```

## Examples

## Best Practices

1. <tip>
2. <tip>
```

## Validation

- [ ] Name is lowercase with hyphens
- [ ] Description < 100 chars
- [ ] Dependencies listed
- [ ] Commands tested
