---
name: aida-glossary
description: Maintain a project glossary and ubiquitous language dictionary. Find inconsistent terminology across requirements and code, propose canonical definitions.
allowed-tools:
  - Bash
  - Read
  - Glob
  - Grep
---

# AIDA Glossary Skill

## Purpose

Maintain a project glossary (ubiquitous language dictionary) by scanning requirements and code for domain terms. Identify inconsistencies, synonyms, and ambiguous usage. Propose canonical definitions that the whole team can align on.

Consistent terminology prevents miscommunication between code, requirements, and conversation. If the code says "item" and the requirements say "ticket" and the UI says "card", everyone is confused.

## When to Use

- When onboarding to a new domain and need to understand the vocabulary
- When requirements use different words for the same concept
- When code and requirements use different terminology
- When a new domain term is introduced and needs a definition
- When the user says "define this", "what do we call X?", "glossary"
- During requirements review to catch ambiguous language

## Workflow

### Step 1: Scan Requirements for Domain Terms

```bash
aida list
```

Read through requirement titles and descriptions, looking for domain-specific terms. Focus on:
- Nouns: entities, concepts, roles (e.g., "sprint", "owner", "backlog")
- Verbs: actions, operations (e.g., "triage", "approve", "deploy")
- Adjectives: states, qualities (e.g., "draft", "approved", "stale")

### Step 2: Scan Code for Domain Terms

Search the codebase for how domain concepts are named:

```bash
# Look at struct/type/class names
# Look at function names
# Look at database table/column names
# Look at API endpoint paths
```

Build a list of terms found in code and compare with terms found in requirements.

### Step 3: Identify Inconsistencies

Look for these problems:

**Synonyms** (different words, same concept):
- Requirements say "ticket", code says "item", UI says "card"
- One file uses "user", another uses "member", another uses "account"

**Homonyms** (same word, different concepts):
- "status" means requirement status in one context, HTTP status in another
- "type" means requirement type vs data type vs user type

**Ambiguous terms** (unclear or overloaded):
- "process" (noun or verb?)
- "handle" (too vague — handle how?)
- "data" (what data specifically?)

**Inconsistent casing/formatting**:
- `sprint_id` vs `sprintId` vs `SprintID`
- "Kanban" vs "kanban" vs "KANBAN"

### Step 4: Propose Canonical Definitions

For each term, propose a definition:

```
**<Term>** (noun/verb/adjective)
  Definition: <one clear sentence>
  Code name: <how it appears in code — struct name, variable name>
  DB name: <table/column name if applicable>
  Synonyms to retire: <list of alternative terms that should be replaced>
  Example: <one sentence using the term correctly>
```

### Step 5: Check for Synonym Conflicts

For each pair of synonyms, decide which term is canonical:

- Which term is most precise?
- Which term is already dominant in the codebase?
- Which term is most familiar to the domain experts?

### Step 6: Record the Glossary

Store glossary definitions. Options:

**Option A: As a requirement** (recommended for AIDA-tracked projects):
```bash
# Create or find the glossary requirement
aida add \
  --title "Project Glossary — Ubiquitous Language" \
  --description "## Glossary

| Term | Definition | Code Name | Retire |
|------|-----------|-----------|--------|
| <term> | <definition> | <code_name> | <synonyms> |
| ... | ... | ... | ... |" \
  --type meta \
  --status approved
```

**Option B: Add individual comments** for discovered terms:
```bash
aida comment add <GLOSSARY-SPEC-ID> "Added: <term> — <definition>. Replaces: <synonyms>"
```

### Step 7: Suggest Renaming

For terms that are used inconsistently, suggest concrete renames:

```
## Suggested Renames

### "ticket" -> "requirement"
- `src/models/ticket.rs` -> rename struct `Ticket` to `Requirement`
- API: `/api/tickets` -> `/api/requirements`
- UI: "Ticket List" -> "Requirement List"
- Impact: <high/medium/low> — <number of files affected>

### "owner" vs "assignee"
- Canonical term: "owner" (already dominant in codebase)
- Files using "assignee": <list>
- Impact: low — 3 files affected
```

Do NOT perform the renames automatically. Present them for the user to approve.

### Step 8: Present the Glossary

```
## Project Glossary

### Core Domain Terms (N)

| Term | Definition | Code Name | Notes |
|------|-----------|-----------|-------|
| ... | ... | ... | ... |

### Inconsistencies Found (N)

1. **<synonym group>**: "<term A>" and "<term B>" mean the same thing
   - Recommendation: Use "<canonical term>" everywhere
   - Files affected: N

2. **<homonym>**: "<term>" means different things in different contexts
   - Context 1: <meaning 1> (in <where>)
   - Context 2: <meaning 2> (in <where>)
   - Recommendation: <rename one or both to be unambiguous>

### Ambiguous Terms (N)

1. **"<term>"** — used in <N> places without clear definition
   - Proposed definition: <definition>

### Suggested Renames (N)
<from Step 7>
```

## Anti-Patterns to Avoid

- **Renaming without consensus**: The glossary proposes, humans decide. Never auto-rename.
- **Over-defining**: Not every variable needs a glossary entry. Focus on domain terms that cause confusion.
- **Ignoring code conventions**: If the code consistently uses one term, don't fight it just because the requirements use another. Meet in the middle.
- **One-time exercise**: A glossary is living. Revisit it when new features introduce new terms.
- **Jargon gatekeeping**: Definitions should be clear to newcomers, not just domain experts.

## CLI Reference

```bash
aida list                              # Scan all requirements for terms
aida show <SPEC-ID>                    # Read requirement details
aida search "<term>"                   # Find term usage in requirements
aida add --title "..." --type meta     # Create glossary requirement
aida comment add <SPEC-ID> "..."       # Add term definitions
aida edit <SPEC-ID> --description "..."  # Update glossary content
```
