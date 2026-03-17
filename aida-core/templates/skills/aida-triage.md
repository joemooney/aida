---
name: aida-triage
description: Structured bug investigation and diagnosis. Systematically reproduce, narrow, root-cause, and plan a fix for a reported bug.
allowed-tools:
  - Bash
  - Read
  - Glob
  - Grep
---

# AIDA Triage Skill

## Purpose

Systematically investigate and diagnose a bug report. Walk through reproduction, narrowing, root cause analysis, impact assessment, and fix strategy. Record all findings on the requirement so knowledge is preserved even if the fix is deferred.

This is NOT "just fix it". Triage is about understanding before acting. A well-triaged bug is half-fixed.

## When to Use

- When a bug is reported and needs investigation
- When a test is failing and the cause is unclear
- When the user says "triage this", "investigate", "why is this broken?"
- Before implementing a fix for a non-trivial bug
- When a bug has been reopened or a previous fix was incomplete

## Workflow

### Step 1: Load the Bug

```bash
aida show <SPEC-ID>
```

Read the title, description, and any existing comments. Note:
- What is the reported symptom?
- What is the expected behavior?
- Are reproduction steps provided?
- What environment/context is specified?

If the bug requirement does not exist yet, create one:

```bash
aida add \
  --title "Bug: <symptom description>" \
  --description "## Reported Symptom
<what the user observed>

## Expected Behavior
<what should happen>

## Reproduction Steps
<if known, otherwise TBD>" \
  --type bug \
  --status draft \
  --priority high
```

### Step 2: Reproduce

Understand the steps to trigger the bug. Try to reproduce it:

```bash
# Run the relevant tests
cargo test <test_name>

# Or try the steps described in the bug
```

If you can reproduce it, record exactly how:

```bash
aida comment add <SPEC-ID> "Reproduced: <exact steps and observed output>"
```

If you cannot reproduce it, record that too:

```bash
aida comment add <SPEC-ID> "Could not reproduce with: <steps tried>. May be environment-specific."
```

### Step 3: Investigate

Search the codebase for the relevant code paths:

```bash
# Search for the component/function mentioned in the bug
aida search "<keywords from bug report>"
```

Use Grep to find the specific code:

```bash
# Find the function, endpoint, or component involved
```

Read the relevant source files to understand the code flow. Trace the execution path from input to the point of failure.

### Step 4: Narrow

Identify the specific component, module, and function responsible:

- Which file(s) contain the bug?
- Which function(s) are involved?
- What is the input that triggers the failure?
- At what point does the actual behavior diverge from the expected?

```bash
aida comment add <SPEC-ID> "Narrowed to: <file>:<function> — <brief explanation of where it diverges>"
```

### Step 5: Root Cause

Determine WHY it fails, not just WHERE. Common root causes:

- **Logic error**: Incorrect condition, wrong operator, off-by-one
- **Missing case**: Unhandled edge case, missing match arm, null/None not checked
- **State corruption**: Shared mutable state, race condition, stale cache
- **Data mismatch**: Schema drift, type mismatch, encoding issue
- **Integration failure**: API contract violation, version incompatibility
- **Configuration**: Wrong default, missing env var, incorrect feature flag

```bash
aida comment add <SPEC-ID> "Root cause: <category> — <detailed explanation>

The bug occurs because <specific mechanism>.
This was introduced by <commit/change if known, or 'unknown'>."
```

### Step 6: Impact Assessment

Assess the severity and blast radius:

- **Who is affected?** All users, specific roles, specific configurations?
- **How often?** Every time, intermittently, only under specific conditions?
- **Data impact?** Is data corrupted, lost, or just displayed incorrectly?
- **Workarounds?** Can users work around it? How painful is the workaround?
- **Security?** Does this expose data or create vulnerabilities?

```bash
aida comment add <SPEC-ID> "Impact assessment:
- Affected users: <scope>
- Frequency: <how often>
- Data impact: <none/display-only/corruption/loss>
- Workaround: <available/painful/none>
- Security: <none/low/high>"
```

Update priority if the assessment changes severity:

```bash
aida edit <SPEC-ID> --priority <high|medium|low>
```

### Step 7: Fix Strategy

Propose a fix approach. Consider:

- **Minimal fix**: What is the smallest change that fixes the bug?
- **Proper fix**: Is there a deeper refactor that prevents this class of bug?
- **Risk**: Could the fix break something else? What needs regression testing?
- **Testing**: How will we verify the fix? What test should be added?

```bash
aida comment add <SPEC-ID> "Fix strategy:
- Approach: <description of the fix>
- Files to change: <list>
- Risk: <low/medium/high> — <why>
- Test plan: <what tests to add/run>
- Estimated effort: <small/medium/large>"
```

### Step 8: Create Fix Task (if needed)

If the fix is not going to be done immediately, create a child task:

```bash
aida add \
  --title "Fix: <concise fix description>" \
  --description "## Root Cause
<from step 5>

## Fix Approach
<from step 7>

## Test Plan
- [ ] <test 1>
- [ ] <test 2>

## Files to Change
- <file 1>
- <file 2>" \
  --type task \
  --status approved \
  --priority <inherit from parent>

aida rel add --from <BUG-SPEC-ID> --to <TASK-SPEC-ID> --type Parent
```

### Step 9: Present Summary

```
## Triage Summary for <SPEC-ID>: <title>

### Status: <Reproduced | Could Not Reproduce | Intermittent>

### Root Cause
<1-2 sentence explanation>

### Impact
- Severity: <Critical | High | Medium | Low>
- Affected: <who/what>
- Workaround: <yes/no — brief description>

### Fix
- Approach: <1-2 sentence description>
- Risk: <Low | Medium | High>
- Effort: <Small | Medium | Large>
- Task: <CHILD-SPEC-ID if created>

### Evidence
- <links to relevant code, logs, or test output>
```

## Anti-Patterns to Avoid

- **Jumping to fix**: Do not start fixing until you have a root cause. Fixing symptoms creates new bugs.
- **Skipping reproduction**: If you cannot reproduce it, you cannot verify the fix. Record what you tried.
- **Blaming the user**: Even if the reproduction steps are wrong, the user experienced something. Understand what.
- **Scope creep**: Triage one bug at a time. If you discover related bugs, create separate requirements.
- **Losing findings**: Every investigation step should be recorded as a comment. If you switch context, the next person needs your findings.

## CLI Reference

```bash
aida show <SPEC-ID>                    # Load bug requirement
aida search "<keyword>"                # Find related requirements/code
aida comment add <SPEC-ID> "..."       # Record findings at each step
aida edit <SPEC-ID> --priority high    # Adjust priority after assessment
aida add --title "..." --type task     # Create fix task
aida add --title "..." --type bug      # Create new bug requirement
aida rel add --from <ID> --to <ID> --type Parent  # Link fix to bug
```
