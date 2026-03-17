---
name: aida-grill
description: Interrogate a requirement or design decision by walking every branch of the decision tree. Use before implementation to catch design gaps.
allowed-tools:
  - Bash
  - Read
  - Glob
  - Grep
---

# AIDA Grill Skill

## Purpose

Systematically interrogate a requirement or design decision before implementation begins. Walk every branch of the decision tree to catch gaps, contradictions, and unstated assumptions.

This is NOT evaluation (scoring quality). This is adversarial design review — asking "what about X?" until every branch is explored.

## When to Use

- Before implementing a complex requirement
- When a design decision has multiple valid approaches
- When requirements feel vague or underspecified
- When the user says "grill this", "poke holes", "what am I missing?"
- Before `/aida-plan` on high-impact features

## Core Approach: Decision Tree Walking

For every design decision, there is a tree of alternatives. Walk every branch:

```
User authentication
├── How do users identify themselves?
│   ├── Email + password
│   │   ├── Password storage? (bcrypt, argon2, scrypt)
│   │   ├── Password requirements? (length, complexity)
│   │   └── Password reset flow?
│   ├── OAuth2 / SSO
│   │   ├── Which providers? (Google, GitHub, SAML)
│   │   ├── Account linking? (same email, different provider)
│   │   └── Fallback if provider is down?
│   └── Magic link / passwordless
│       ├── Email delivery reliability?
│       └── Link expiry time?
├── How are sessions managed?
│   ├── JWT vs server-side sessions
│   ├── Token refresh strategy
│   └── Concurrent session limits?
├── What happens on failure?
│   ├── Rate limiting? (how many attempts)
│   ├── Account lockout? (temporary or permanent)
│   └── Notification to user on failed attempts?
└── What's out of scope?
    ├── MFA (explicitly deferred or never?)
    └── Admin impersonation?
```

## Workflow

### Step 1: Load the Subject

If a SPEC-ID is provided:
```bash
aida show <SPEC-ID>
```

If it's a general design question, ask the user to describe it in 2-3 sentences.

### Step 2: Identify the Root Decisions

List the 3-5 top-level decisions implied by the requirement. For each, ask:

- What are the options?
- What are the constraints?
- What has already been decided vs what's open?

### Step 3: Walk Each Branch

For each decision branch:

1. **State the question** clearly
2. **List the alternatives** (at least 2, ideally 3+)
3. **Ask the user** which they prefer and why
4. **Challenge the choice** — what's the downside? What breaks?
5. **Record the decision** before moving to the next branch

**Do not move past a branch until it's resolved.** If the user says "I don't know", that's valuable — flag it as an open question.

### Step 4: Explore Edge Cases

For each resolved branch, ask:
- What happens at the boundaries? (zero items, max items, empty string)
- What happens on failure? (network down, disk full, invalid input)
- What happens concurrently? (two users doing this at once)
- What happens over time? (data grows, users scale, requirements change)

### Step 5: Check for Contradictions

Review all decisions together:
- Do any conflict with each other?
- Do any conflict with existing requirements?
- Are there implicit assumptions that should be explicit?

```bash
aida search "<keywords from the decisions>"
```

### Step 6: Record Findings

For each significant finding:

```bash
# Record design decisions
aida comment add <SPEC-ID> "Decision: <branch> — chose <option> because <reason>"

# Record open questions
aida comment add <SPEC-ID> "Open question: <what's unresolved and why>"

# Record risks
aida comment add <SPEC-ID> "Risk: <what could go wrong>"

# Create child requirements for discovered scope
aida add --title "<discovered need>" --description "<details>" --type task --status draft
aida rel add --from <SPEC-ID> --to <CHILD-ID> --type Parent
```

### Step 7: Present Summary

Format the findings as:

```
## Grill Results for <SPEC-ID>

### Decisions Made (N)
1. <branch>: chose <option> — <one-line reason>
2. ...

### Open Questions (N)
1. <question> — needs <who/what> to resolve
2. ...

### Risks Identified (N)
1. <risk> — mitigation: <strategy or "none yet">
2. ...

### Scope Additions (N)
1. <CHILD-ID>: <title> — discovered during <branch> exploration
2. ...
```

## Anti-Patterns to Avoid

- **Don't accept the first answer.** Push back at least once on every decision.
- **Don't skip "obvious" branches.** Obvious decisions often have non-obvious consequences.
- **Don't propose solutions.** Ask questions. The user decides.
- **Don't stop at the happy path.** Failure modes and edge cases are where designs break.
- **Don't rush.** Thoroughness is the point. A 20-minute grill saves days of rework.

## Example Interaction

```
User: /aida-grill FR-042

AI: FR-042: "OAuth2 token validation"
    Let me walk the decision tree.

    1. Where is the JWKS endpoint configured?
       a) Environment variable
       b) Database setting
       c) Hardcoded
       Which do you prefer?

User: Environment variable

AI: OK — ENV var. What happens if it's not set?
    a) Fail to start (strict)
    b) Fall back to a default
    c) Disable auth entirely

User: Fail to start

AI: Good, strict. Now — what happens if the JWKS endpoint is unreachable
    at runtime? (DNS failure, endpoint down, certificate expired)
    a) Reject all tokens (deny by default)
    b) Cache the last known keys and use those
    c) Allow tokens through (fail open)

User: Cache last known keys

AI: For how long? If the keys rotate and you're serving stale keys,
    you'll accept tokens signed with compromised keys.

User: Good point. Cache for 1 hour max, then deny.

AI: Decision recorded. Moving to the next branch...
```

## CLI Reference

```bash
aida show <SPEC-ID>                    # Load requirement
aida search "<keyword>"                # Find related requirements
aida comment add <SPEC-ID> "..."       # Record decisions/questions/risks
aida add --title "..." --type task     # Create child for discovered scope
aida rel add --from <ID> --to <ID> --type Parent
```
