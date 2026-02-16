# Project Status

Show current project status and requirements summary.

## Current Status

!`aida list --format summary 2>/dev/null || echo "No database found"`

## Instructions

1. Run `aida list --status approved` to show approved requirements
2. Run `aida list --status draft` to show draft requirements needing review
3. Summarize the current state of the project

## Output Format

```
## Project Status

### Approved Requirements (Ready for Implementation)
- [SPEC-ID] Title

### Draft Requirements (Needing Review)
- [SPEC-ID] Title

### Recently Completed
- [SPEC-ID] Title
```
