---
name: summarize
description: Summarize long content, conversations, or documents
always: false
---

# Content Summarization Skill

This skill provides guidance on summarizing various types of content effectively.

## Overview

Summarization is useful for:
- Condensing long conversations
- Creating executive summaries of documents
- Extracting key points from meetings
- Generating TL;DR versions of articles

## When to Summarize

Summarize when:
- Content exceeds a reasonable length (>1000 words)
- User asks for a summary or "TL;DR"
- You need to preserve information in memory
- Preparing reports or updates
- Extracting action items from discussions

## Summarization Techniques

### 1. Executive Summary

For formal documents or reports:

```
Structure:
1. **Purpose**: What is this about? (1 sentence)
2. **Key Findings**: Main points (3-5 bullets)
3. **Recommendations**: Next steps (1-2 bullets)
4. **Impact**: Why it matters (1 sentence)

Example:
# Executive Summary: Project Status Update

## Purpose
This report summarizes the nanobot Rust migration progress for January 2024.

## Key Findings
- Core framework completed (100%)
- Skills system implemented (80%)
- Test coverage at 85%
- Performance improved by 60%

## Recommendations
- Continue with Phase 2 implementation
- Increase test coverage to 90%

## Impact
Migration is on track for Q2 2024 release.
```

### 2. Bullet Points

For quick reference:

```
Key Points:
- First important point
- Second important point
- Third important point
- Fourth important point
- Fifth important point

Action Items:
- [ ] Action 1
- [ ] Action 2
- [ ] Action 3
```

### 3. Hierarchical Summary

For complex topics:

```
# Main Topic

## Subtopic 1
- Point 1.1
- Point 1.2

## Subtopic 2
- Point 2.1
- Point 2.2

## Subtopic 3
- Point 3.1
- Point 3.2
```

### 4. Conversation Summary

For chat histories:

```
# Conversation Summary (2024-01-15)

## Participants
- User (project manager)
- Assistant

## Topics Discussed
1. Project timeline
2. Resource allocation
3. Risk assessment

## Key Decisions
- Deadline extended to March 15
- Added 2 team members
- Increased budget by 15%

## Action Items
- User: Update project plan
- User: Schedule team meeting
- Assistant: Send summary to stakeholders

## Next Steps
- Review updated timeline
- Finalize resource allocation
```

## Summarization Process

When asked to summarize:

1. **Read/Analyze**: Understand the full content
2. **Identify Key Points**: Extract main ideas
3. **Organize**: Group related points
4. **Condense**: Rewrite in fewer words
5. **Review**: Ensure accuracy and completeness

## Best Practices

1. **Preserve Meaning**: Don't lose critical information
2. **Be Objective**: Stick to facts, avoid interpretation
3. **Use Clear Structure**: Help readers scan quickly
4. **Include Numbers**: Quantify when possible
5. **Add Context**: Explain why points matter
6. **Cite Sources**: Reference original content

## Length Guidelines

| Original Length | Summary Length |
|-----------------|----------------|
| < 500 words | 50-100 words |
| 500-2000 words | 100-200 words |
| 2000-5000 words | 200-400 words |
| > 5000 words | 400-800 words |

## Special Cases

### Technical Content

For code or technical docs:

```
# Technical Summary

## Overview
Brief description of the technical topic

## Key Components
- Component 1: Description
- Component 2: Description
- Component 3: Description

## Implementation Notes
- Important technical details
- Configuration requirements
- Dependencies

## Code Example
```language
// Brief example if helpful
```

## References
- Link to documentation
- Related resources
```

### Meeting Notes

```
# Meeting Summary: [Meeting Title]

**Date**: YYYY-MM-DD
**Attendees**: List of participants
**Duration**: X minutes

## Agenda
1. Topic 1
2. Topic 2
3. Topic 3

## Discussion Points
- Point 1: Summary
- Point 2: Summary
- Point 3: Summary

## Decisions Made
- Decision 1
- Decision 2

## Action Items
- [ ] Task 1 - @assignee - Due date
- [ ] Task 2 - @assignee - Due date

## Next Meeting
Date: YYYY-MM-DD
Topics: Preview of next meeting
```

## Using with Memory

Store summaries in memory for future reference:

```
# Store in MEMORY.md
Add summary under appropriate section

# Store in HISTORY.md
Add dated entry with summary reference

Example:
## 2024-01-15
- Summarized project requirements document
- Key requirements: [link to full summary]
- Stored in MEMORY.md under "Project Requirements"
```

## Example Workflow

### User Request:
"Summarize our conversation about the Rust migration"

### Assistant Process:
1. Review conversation history
2. Identify key topics discussed
3. Extract decisions and action items
4. Organize into structured format
5. Present summary to user
6. Optionally store in memory

### Output:
```
# Conversation Summary: Rust Migration Discussion

## Topics Covered
1. Current progress (Phases 1-4 complete)
2. Missing features (Skills system, providers)
3. Timeline and next steps

## Key Decisions
- Prioritize Skills system implementation
- Add DeepSeek and Gemini providers
- Implement CLI channels command

## Action Items
- [ ] Complete Skills system framework
- [ ] Add 3 built-in skills
- [ ] Extend provider support

## Timeline
- Iteration 1: 2 weeks
- Iteration 2: 2 weeks
- Iteration 3: 1 week

## Status
Migration is 70% complete, on track for Q2 2024 release.
```

## Tips for Quality Summaries

1. **Start Strong**: Lead with the most important point
2. **Use Active Voice**: "Team completed migration" not "Migration was completed"
3. **Be Specific**: "Reduced latency by 40%" not "Improved performance"
4. **Avoid Jargon**: Use plain language when possible
5. **Include Context**: Help readers understand significance
6. **Proofread**: Ensure accuracy and clarity

## Common Mistakes

❌ Too detailed (not a summary anymore)
❌ Too vague (loses meaning)
❌ Missing key points
❌ Adding new information
❌ Changing meaning
❌ Poor organization

## Related Skills

- **memory**: Store summaries for long-term reference
- **cron**: Schedule regular summary reports
- **github**: Summarize PRs, issues, or commits
