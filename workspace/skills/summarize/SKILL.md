---
name: summarize
description: Summarize long content, conversations, or documents
always: false
---

# Content Summarization Skill

## When to Summarize

- Content >1000 words or user requests "TL;DR"
- Need to preserve information in memory
- Extracting key decisions and action items

## Formats

### Executive Summary
```
## Purpose
One sentence about what this is about.

## Key Points
- 3-5 main findings

## Next Steps
- 1-2 recommendations

## Impact
Why it matters (1 sentence)
```

### Bullet Summary
```
Key Points:
- Point 1
- Point 2
- Point 3

Action Items:
- [ ] Task 1
- [ ] Task 2
```

### Conversation Summary
```
## Topics
1. Topic 1
2. Topic 2

## Decisions
- Decision 1
- Decision 2

## Action Items
- [ ] Task - @assignee

## Status
Current state summary
```

## Process

1. **Analyze** - Understand full content
2. **Extract** - Identify key ideas
3. **Organize** - Group related points
4. **Condense** - Rewrite concisely
5. **Verify** - Check accuracy

## Best Practices

✅ Preserve meaning, use active voice
✅ Be specific: "40% faster" not "improved"
✅ Include context and numbers
✅ Use clear structure

❌ Too detailed or too vague
❌ Missing key points
❌ Adding new information

## Length Guidelines

| Original | Summary |
|----------|---------|
| <500 words | 50-100 words |
| 500-2000 | 100-200 words |
| 2000-5000 | 200-400 words |
| >5000 | 400-800 words |

## Memory Integration

Store summaries in `/memories/` for future reference. Use memory tool to persist key information.

## Related Skills

- **memory** - Store summaries
- **cron** - Schedule reports
- **github** - Summarize PRs/issues
