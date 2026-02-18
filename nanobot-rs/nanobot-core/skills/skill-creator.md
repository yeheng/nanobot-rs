---
name: skill-creator
description: Helper skill for creating new nanobot skills with proper format and structure
always: false
bins:
  - jq
---

# Skill Creator

This skill helps create new nanobot skills with the correct format and structure.

## Skill File Structure

Every skill is a Markdown file with YAML frontmatter:

```markdown
---
name: skill-name
description: Brief description of what this skill does
always: false
bins:
  - required-binary-1
  - required-binary-2
env_vars:
  - REQUIRED_ENV_VAR
---

# Skill Title

Detailed skill content here...
```

## Frontmatter Fields

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique skill identifier (lowercase, hyphens) |
| `description` | Yes | Brief description shown in skill listings |
| `always` | No | If true, always load full content (default: false) |
| `bins` | No | List of required binary commands |
| `env_vars` | No | List of required environment variables |

## Creating a New Skill

### Step 1: Choose a Name

Good skill names:
- Are lowercase with hyphens: `my-skill`
- Are descriptive: `github-ops`, `docker-manager`
- Are concise: `weather`, `tmux`

### Step 2: Identify Dependencies

Check what tools your skill needs:

```bash
# Check if binary exists
which <binary-name>

# Check if env var is set
echo $ENV_VAR_NAME
```

### Step 3: Create the File

```bash
# Create skill in user skills directory
cat > ~/.nanobot/skills/my-skill.md << 'EOF'
---
name: my-skill
description: My custom skill description
always: false
bins:
  - my-tool
---

# My Skill

Content goes here...
EOF
```

### Step 4: Test the Skill

```bash
# Reload skills (or restart nanobot)
# Verify skill is loaded
```

## Skill Content Guidelines

### Include These Sections

1. **Prerequisites** - What's needed to use the skill
2. **Common Operations** - Frequently used commands/patterns
3. **Examples** - Practical usage examples
4. **Best Practices** - Tips for effective use

### Use Code Blocks

````markdown
```bash
# Commands with comments
command --option value
```
````

### Use Tables for Reference

````markdown
| Option | Description |
|--------|-------------|
| `-v` | Verbose output |
| `-q` | Quiet mode |
````

## Template

Use this template for new skills:

```markdown
---
name: <skill-name>
description: <Brief description>
always: false
bins:
  - <binary1>
  - <binary2>
env_vars:
  - <ENV_VAR1>
---

# <Skill Name>

<Overview of what this skill does>

## Prerequisites

<List what's needed>

## Common Operations

### <Category 1>

```bash
<commands>
```

### <Category 2>

```bash
<commands>
```

## Examples

### <Example 1>

<description>

```bash
<commands>
```

## Best Practices

1. <tip 1>
2. <tip 2>
3. <tip 3>

## Notes

<Any additional notes>
```

## Validation Checklist

Before publishing a skill, verify:

- [ ] Name is lowercase with hyphens
- [ ] Description is concise (< 100 chars)
- [ ] All dependencies are listed
- [ ] Commands are tested and working
- [ ] Examples are practical
- [ ] Content is well-formatted

## Example: Creating a Docker Skill

```bash
cat > ~/.nanobot/skills/docker.md << 'SKILL_EOF'
---
name: docker
description: Docker container management operations
bins:
  - docker
---

# Docker Skill

Manage Docker containers, images, and volumes.

## Common Operations

### Containers

\`\`\`bash
# List running containers
docker ps

# List all containers
docker ps -a

# Run a container
docker run -d --name myapp nginx
\`\`\`
SKILL_EOF
```
