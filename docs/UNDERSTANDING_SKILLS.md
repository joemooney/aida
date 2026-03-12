# Skills versus Commands in Claude Code

These two constructs serve **different roles** in Claude Code and have a clean separation of concerns, though they can (and often should) reference each other.

## The Conceptual Difference

**Skills** are *passive knowledge* — they tell Claude *how to think and act* when working in a domain. Claude reads them and internalizes guidance. They're applied contextually, either by Claude recognizing relevance or via explicit `read_me` instructions.

**Commands** are *active triggers* — they define a repeatable workflow that a user explicitly invokes (e.g., `/aida-capture`). They're procedural: "when the user runs this, do these steps."

Think of it this way:
- A **skill** is like a domain expert's handbook
- A **command** is like a runbook that says "execute this procedure"

## How They Relate

```
User runs /aida-capture
        ↓
Command defines the workflow steps
        ↓
Command instructs Claude to load/follow the aida-capture skill
        ↓
Skill provides the domain knowledge for HOW to do it well
```

The **command references the skill** — not the other way around. The command says "for this workflow, apply the aida-capture skill." The skill itself stays agnostic of how it was invoked.

## Practical Structure for Aida

```
.claude/
  commands/
    aida-capture.md       ← the /aida-capture slash command
  skills/
    aida-capture/
      SKILL.md            ← the capture domain knowledge
```

**`aida-capture.md` (command)** should:
- Define the trigger/intent ("User wants to capture a new requirement")
- Specify the steps to execute
- Explicitly reference the skill: `"Follow the guidance in .claude/skills/aida-capture/SKILL.md"`
- Handle input/output specifics (what args, what files to touch)

**`aida-capture/SKILL.md` (skill)** should:
- Define what good capture looks like
- Cover edge cases, formatting rules, validation heuristics
- Be *command-agnostic* — Claude might apply this skill during a freeform conversation too, not just via the command
- Not reference the command at all

## Why the Skill Shouldn't Reference the Command

The skill is reusable context. If you later add an `/aida-batch-import` command, it might also load the capture skill. If the skill said "invoke `/aida-capture`" you'd have a circular or confusing dependency. Skills should be invocation-neutral.

## Summary

| Aspect | Command | Skill |
|--------|---------|-------|
| Triggered by | User explicitly | Context or command |
| References | The skill | Nothing procedural |
| Contains | Workflow steps | Domain knowledge |
| Reusability | Single workflow | Multiple workflows |

So yes — your `aida-capture` command should load the `aida-capture` skill as part of its execution. That's the right pattern.

