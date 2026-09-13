---
description: "Capture durable project memory in AIDA as work unfolds."
---
# AIDA Memory Capture

Keep AIDA's project memory current while the conversation is still fresh —
capture as you go; do not leave durable decisions only in chat.

## Instructions

Follow the workflow in `.claude/skills/aida-memory-capture/SKILL.md`:

1. Check for existing memory first:
   ```bash
   AIDA_AGENT_OUTPUT=toon aida search "<topic>"
   ```
   Prefer updating the existing spec when it already covers the topic.
2. Update or add — choose the narrowest honest type and status:
   ```bash
   aida comment add <SPEC-ID> "..."
   aida edit <SPEC-ID> --status in-progress
   aida add --type task --status draft --title "..." --description "..."
   ```
3. Link code when applicable with a nearby trace comment
   (`// trace:<SPEC-ID> | ai:<tool>`).
4. Capture facts that save the next session from rediscovering context (why
   a choice was made, a constraint, a known follow-up, a user preference);
   skip transient narration and output already obvious from the code.

Use when a decision, requirement, caveat, or note should outlive the chat.
