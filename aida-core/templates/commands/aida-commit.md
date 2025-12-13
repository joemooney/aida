# AIDA Commit

Commit staged changes with automatic requirement linking.

## Usage

```
/aida-commit [message]
```

## Instructions

Follow the workflow in `.claude/skills/aida-commit.md`:

1. Analyze staged changes and extract requirement traces
2. Check for untraced implementation code
3. Offer to create requirements for untraced work
4. Create commit with requirement links in message
5. Update linked requirement statuses
