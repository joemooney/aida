---
name: aida-standup
description: Generate daily standup summary from recent commits and requirement status changes. Quick overview of yesterday's work and today's plan.
allowed-tools:
  - Bash
---
<!-- AIDA Generated: v2.0.0 | checksum:cc53f1ce | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


# AIDA Standup Skill

## Purpose

Generate a daily standup summary by analyzing recent git commits, extracting linked requirement IDs, and presenting what was done and what's planned next.

## When to Use

Use this skill when:
- User asks for a standup summary or daily status
- User says "what did I do yesterday?" or "standup"
- At the start of a work day to review progress

## Recent Activity

!`git log --oneline --since="yesterday" 2>/dev/null | head -10 || echo "no recent commits"`

## Workflow

### Step 1: Gather Yesterday's Commits

```bash
# Commits since yesterday
git log --oneline --since="yesterday" --author="$(git config user.name)" 2>/dev/null

# If no commits yesterday, expand to last 2 days
git log --oneline --since="2 days ago" --author="$(git config user.name)" 2>/dev/null | head -10
```

### Step 2: Extract Requirement IDs

Parse commit messages for requirement references:

```bash
# Extract SPEC-IDs from commit messages
git log --since="yesterday" --format="%s" 2>/dev/null | grep -oE '[A-Z]+-[0-9]+' | sort -u
```

### Step 3: Check Requirement Statuses

For each extracted SPEC-ID:

```bash
aida show <SPEC-ID> 2>/dev/null
```

### Step 4: Check Current Work

```bash
# In-progress requirements
aida list --status in-progress 2>/dev/null | head -10

# Recently completed
aida list --status completed 2>/dev/null | head -5
```

### Step 5: Generate Standup

```
## Daily Standup — <date>

### Done (Yesterday)
- [FR-0042] Login validation — completed
- [BUG-0023] Fixed null response handling
- 3 commits pushed

### In Progress (Today)
- [FR-0043] Password strength requirements
- [FR-0050] User registration flow

### Blockers
- (none identified)

### Metrics
- Commits: N
- Requirements completed: M
- Requirements in progress: P
```

## CLI Reference

```bash
aida list --status in-progress                   # Current work
aida list --status completed                     # Recently done
aida show <SPEC-ID>                              # Requirement details
```