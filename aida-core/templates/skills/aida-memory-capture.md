---
name: aida-memory-capture
description: Capture durable project memory in AIDA as work unfolds.
allowed-tools:
  - Bash
  - Grep
  - Read
---

# AIDA Memory Capture

## Purpose

Keep AIDA's project memory current while the conversation is still fresh.
Capture as you go; do not leave durable decisions only in chat.

## When to Use

Use this skill when:
- The user makes a decision that future work should know
- You discover a requirement or caveat not already captured
- An existing requirement needs a note, status update, or correction
- You finish a meaningful change and the memory does not reflect it yet

## Workflow

### 1. Check For Existing Memory

```bash
AIDA_AGENT_OUTPUT=toon aida search "<topic>"
```

Prefer updating the existing spec when it already covers the topic.

### 2. Update Or Add

For a note on existing memory:

```bash
aida comment add <SPEC-ID> "..."
```

For status or description changes:

```bash
aida edit <SPEC-ID> --status in-progress
aida edit <SPEC-ID> --description "..."
```

For new memory:

```bash
aida add --type task --status draft --title "..." --description "..."
```

Choose the narrowest honest type and status. Use `draft` when the thought
needs later refinement.

### 3. Link Code When Applicable

For implementation tied to a spec, add a nearby trace comment:

```rust
// trace:<SPEC-ID> | ai:<tool>
```

## Capture Standard

Capture facts that will save the next session from rediscovering context:
- Why a design choice was made
- A constraint or edge case
- A known follow-up
- A user preference about the project

Skip transient narration, raw command output, and details already obvious
from the code.
