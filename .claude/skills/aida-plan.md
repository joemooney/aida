# AIDA Planning Skill

## Purpose

Plan the implementation of an approved requirement before coding begins. This ensures implementation is well-thought-out, decomposed into manageable pieces, and the plan is recorded in the requirements database.

## When to Use

Use this skill when:
- User says "plan <SPEC-ID>" or "plan implementation of <requirement>"
- A requirement is in `Approved` status and needs planning before implementation
- `/aida-implement` is invoked on a requirement that hasn't been planned yet
- User wants to decompose a large requirement into child requirements

## Core Principles

### Think Before Code
Planning separates design decisions from implementation. This allows for:
- Reviewing approach before committing effort
- Identifying risks and unknowns early
- Creating a clear implementation roadmap
- Breaking down complex work into manageable pieces

### Documented Plans
All planning decisions should be captured in the requirements database as:
- Child requirements for sub-tasks
- Comments for design decisions and trade-offs
- Status transition to `Planned` when complete

## Workflow

### Step 1: Load Requirement Context

Fetch the requirement details:

```bash
aida show <SPEC-ID>
```

Display to user:
- SPEC-ID and title
- Current description
- Status, priority, type
- Related requirements (parent/child, links)
- Any existing comments

Verify the requirement is in `Approved` status. If not, inform the user:
- `Draft`: Needs approval first
- `Planned`: Already planned, proceed to `/aida-implement`
- `In Progress` or `Completed`: Already being/been implemented

### Step 2: Analyze Scope

Examine the requirement to understand:
1. What files will need to be created or modified?
2. What external dependencies are involved?
3. Are there any architectural decisions to make?
4. What are the edge cases and error scenarios?
5. Are there any unknowns or risks?

For each significant unknown, note it as a question to resolve during planning.

### Step 3: Decompose into Child Requirements

If the requirement is complex, break it into child requirements:

```bash
# Create child requirement for each logical unit of work
aida add \
  --title "Component: User input validation" \
  --description "Validate user input for..." \
  --type task \
  --status draft

# Link as child
aida rel add --from <PARENT-ID> --to <CHILD-ID> --type Parent
```

Guidelines for decomposition:
- Each child should be implementable in a focused session
- Children should have clear boundaries
- Avoid too many children (3-7 is usually good)
- Order children by implementation dependency

### Step 4: Document Design Decisions

Record any significant design decisions:

```bash
aida comment add <SPEC-ID> "Design: Using async/await pattern because..."
aida comment add <SPEC-ID> "Decision: Chose HashMap over BTreeMap for O(1) lookup"
aida comment add <SPEC-ID> "Risk: External API rate limiting may need handling"
```

### Step 5: Identify File Changes

List the files that will be modified or created:

```bash
aida comment add <SPEC-ID> "Files to modify:
- src/models.rs: Add new struct
- src/handlers.rs: Add endpoint
- src/tests/mod.rs: Add unit tests"
```

### Step 6: Mark as Planned

When planning is complete:

```bash
aida edit <SPEC-ID> --status planned
aida comment add <SPEC-ID> "Planning complete. Ready for implementation."
```

If child requirements were created, approve them:

```bash
aida edit <CHILD-ID> --status approved
```

### Step 7: Present Plan to User

Summarize for the user:
1. Overview of implementation approach
2. List of child requirements created
3. Key design decisions made
4. Files that will be affected
5. Any risks or unknowns identified

Ask if they want to proceed to implementation with `/aida-implement`.

## Status Transitions

During planning, requirements transition:

1. **Approved** -> **Planned** (when planning is complete)

Child requirements created during planning start as:
- **Draft** -> **Approved** (when ready for implementation)

## Integration with /aida-implement

When `/aida-implement` is invoked on a requirement:
1. Check the status
2. If `Approved` (not `Planned`), suggest running `/aida-plan` first
3. If `Planned`, proceed with implementation

## CLI Reference

```bash
# Show requirement
aida show <SPEC-ID>

# Check status
aida show <SPEC-ID> | grep Status

# Create child requirement
aida add --title "..." --description "..." --type task --status draft

# Link child to parent
aida rel add --from <PARENT-ID> --to <CHILD-ID> --type Parent

# Add design comment
aida comment add <SPEC-ID> "Design: ..."

# Mark as planned
aida edit <SPEC-ID> --status planned

# Approve child requirements
aida edit <CHILD-ID> --status approved

# List children of a requirement
aida show <SPEC-ID>  # Shows relationships section
```

## Example Planning Session

User invokes: `/aida-plan FR-0100`

1. Fetch requirement:
   ```
   FR-0100: Add user authentication
   Status: Approved
   Description: The system shall support user authentication with username/password
   ```

2. Decompose:
   - FR-0100-A: Database schema for users (task)
   - FR-0100-B: Password hashing module (task)
   - FR-0100-C: Login endpoint (task)
   - FR-0100-D: Session management (task)

3. Document decisions:
   - "Using argon2 for password hashing"
   - "JWT tokens for session management"
   - "30-minute token expiry"

4. Identify files:
   - src/models/user.rs (new)
   - src/auth/mod.rs (new)
   - src/routes/auth.rs (new)
   - migrations/001_users.sql (new)

5. Mark as planned and present summary
