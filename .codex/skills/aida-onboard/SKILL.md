---
name: aida-onboard
description: Interactive project onboarding for new team members. Detect project type, summarize architecture, show requirement stats, and suggest first tasks.
allowed-tools:
  - Bash
  - Read
  - Glob
  - Grep
---
<!-- AIDA Generated: v2.0.0 | checksum:e0265bb7 | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


# AIDA Project Onboarding Skill

## Purpose

Guide new team members through a project by detecting its structure, summarizing architecture, showing requirements status, and suggesting where to start.

## When to Use

Use this skill when:
- A new developer joins and asks "how does this project work?"
- User says "onboard me" or "help me get started"
- User wants an overview of the project state and priorities

## Workflow

### Step 1: Detect Project Environment

```bash
# Detect project type
ls Cargo.toml package.json pyproject.toml setup.py go.mod 2>/dev/null

# Check for AIDA (distributed git-canonical is the default; .aida/config.toml
# + the .aida-store worktree are the markers)
ls .aida/config.toml .aida-store 2>/dev/null

# Read project instructions
cat CLAUDE.md 2>/dev/null | head -50
```

### Step 2: Summarize Architecture

Read key project files to understand the structure:

```bash
# Project structure overview
ls -la src/ lib/ app/ 2>/dev/null
```

Present a brief architecture summary:
- Project type and language
- Key directories and their purpose
- Build system and dependencies
- Testing framework

### Step 3: Show Requirements Overview

```bash
# Requirements overview (open work first; `--all` adds completed)
aida list 2>/dev/null || echo "No AIDA store found — run 'aida init' first"

# What's still to do
aida list open 2>/dev/null

# Priority items
aida list --status approved --priority high 2>/dev/null | head -10
```

### Step 4: Show Recent Activity

```bash
# Recent commits
git log --oneline -10 2>/dev/null

# Recent requirement changes
aida list --status in-progress 2>/dev/null | head -5
aida list --status completed 2>/dev/null | head -5
```

### Step 5: Present Onboarding Summary

```
## Project Onboarding: <project-name>

### Architecture
- Type: <Rust/Python/TypeScript/etc.>
- Key modules: <list>
- Build: <command>
- Tests: <command>

### Requirements Status
- Total: N requirements
- In Progress: X
- Approved (ready to work): Y
- Draft (needs review): Z

### Suggested First Tasks
1. [SPEC-ID] <high-priority approved requirement>
2. [SPEC-ID] <good-first-issue type task>
3. Review CLAUDE.md for development conventions

### Useful Commands
- `aida list` — Browse all requirements
- `aida show <ID>` — View requirement details
- `/aida-implement <ID>` — Start working on a requirement
- `/aida-plan <ID>` — Plan implementation approach
```

### Step 6: Offer Next Steps

Ask the user what they'd like to do:
1. **Explore a feature area**: Deep dive into a specific feature
2. **Start on a task**: Pick up an approved requirement
3. **Review requirements**: Browse and understand the spec landscape
4. **Run the project**: Build and test locally

## Best Practices

- Keep the summary concise — new developers need orientation, not documentation
- Highlight the highest-priority approved requirements as starting points
- Point out the CLAUDE.md and any OVERVIEW.md for deeper reading