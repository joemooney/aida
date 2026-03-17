---
name: aida-architecture
description: Review codebase architecture health. Identify structural issues like god objects, circular dependencies, shallow modules, and orphaned code. Suggest improvements aligned with requirements.
allowed-tools:
  - Bash
  - Read
  - Glob
  - Grep
---

# AIDA Architecture Skill

## Purpose

Review the health of the codebase architecture. Identify structural problems (god objects, circular dependencies, shallow modules, orphaned code) and compare the actual code structure against the domain model expressed in requirements. Create actionable improvement requirements when issues are found.

This is a diagnostic skill, not a refactoring skill. It finds problems and creates requirements. The actual refactoring happens later via `/aida-implement`.

## When to Use

- Periodically as a health check (e.g., every few sprints)
- Before a major new feature to assess if the architecture can support it
- When the codebase "feels wrong" but nobody can articulate why
- When the user says "review architecture", "code health", "tech debt audit"
- After rapid development sprints to check for accumulated debt
- When onboarding to an unfamiliar codebase

## Workflow

### Step 1: Explore the Codebase Structure

Map out the top-level directory and module organization:

```bash
# Top-level structure
ls -d */

# Source code tree (directories only)
find . -type d -not -path '*/\.*' -not -path '*/node_modules/*' -not -path '*/target/*' | head -50
```

Read key configuration files to understand the project's build and dependency structure:

```bash
# Rust: Cargo.toml, workspace members
# Node: package.json, tsconfig.json
# Python: pyproject.toml, setup.py
```

Document the module/package boundaries you find.

### Step 2: Identify Deep Modules (Good)

Deep modules have a small interface but a large, complex implementation. These are GOOD design — they hide complexity behind a simple API.

Look for:
- Small public API surface (few public functions/methods)
- Large implementation (many private functions, significant logic)
- Clear single responsibility
- Good encapsulation (internals not leaked)

```bash
# Find files and check their size and public API surface
# Look for modules with few pub/export declarations relative to total code
```

Note these as positive examples of good architecture.

### Step 3: Identify Shallow Modules (Bad)

Shallow modules have a large interface but trivial implementation. These add complexity without hiding it — they are pass-through layers that just forward calls.

Look for:
- Many public functions that are thin wrappers
- "Manager", "Helper", "Utils" classes that just delegate
- Layers that add no logic, just re-export or transform trivially

### Step 4: Check for Circular Dependencies

Look for circular imports/dependencies between modules:

```bash
# For Rust: check mod.rs / lib.rs for cross-module dependencies
# For TypeScript: check import statements for cycles
# For any language: grep for imports between peer modules
```

Circular dependencies indicate unclear module boundaries. Document which modules depend on each other in both directions.

### Step 5: Check for God Objects/Files

Find files that are too large or have too many responsibilities:

```bash
# Find large files (>500 lines as warning, >1000 as problem)
find . -name '*.rs' -o -name '*.ts' -o -name '*.tsx' -o -name '*.py' | \
  xargs wc -l 2>/dev/null | sort -rn | head -20
```

For each large file:
- How many distinct concerns does it handle?
- Could it be split into coherent sub-modules?
- Is the size justified (e.g., generated code, large match statements)?

### Step 6: Check for Orphaned Code

Look for code that has no callers:

```bash
# Find public functions and check if they are called anywhere
# Look for dead feature flags
# Check for unused imports/dependencies
```

For Rust projects:
```bash
# Compiler warnings about dead code
cargo build 2>&1 | grep "warning.*dead_code\|warning.*unused"
```

### Step 7: Compare Architecture Against Domain

Load the requirements to understand the domain model:

```bash
aida list
```

Ask:
- **Does the code structure mirror the domain?** If requirements talk about "sprints", "requirements", and "views", are those reflected in the module structure?
- **Are domain concepts scattered?** Is "sprint" logic spread across 10 files, or concentrated in a sprint module?
- **Are infrastructure concerns separated?** Is database access mixed into business logic, or isolated behind a storage interface?
- **Do module boundaries match requirement boundaries?** Could you implement a requirement by changing one module, or do all requirements touch all modules?

### Step 8: Create Improvement Requirements

For each significant finding, create a requirement:

```bash
aida add \
  --title "Arch: <concise description of the improvement>" \
  --description "## Problem
<what is wrong and why it matters>

## Evidence
- <file:line or module reference>
- <metrics: lines, dependencies, etc.>

## Proposed Improvement
<what to do about it>

## Risk
<what could go wrong during refactoring>

## Effort
<small/medium/large>" \
  --type task \
  --status draft \
  --tags "tech-debt,architecture" \
  --priority <high|medium|low>
```

### Step 9: Present the Architecture Report

```
## Architecture Health Report

### Summary
- Overall health: <Good | Fair | Needs Attention | Critical>
- Files analyzed: <N>
- Issues found: <N>

### Strengths
1. **<module/pattern>**: <why it's good>
2. ...

### Deep Modules (Good Design)
1. **<module>** — <small API, large implementation, clear responsibility>
2. ...

### Issues Found

#### God Objects (N)
| File | Lines | Concerns | Priority |
|------|-------|----------|----------|
| <file> | <N> | <list> | <high/med/low> |

#### Shallow Modules (N)
| Module | Public API | Implementation | Issue |
|--------|-----------|----------------|-------|
| <module> | <N functions> | <trivial> | <pass-through / no logic> |

#### Circular Dependencies (N)
1. **<module A> <-> <module B>**: <what they share, why it's circular>
2. ...

#### Orphaned Code (N)
1. **<function/module>**: <last meaningful use, safe to remove?>
2. ...

#### Domain Misalignment (N)
1. **<domain concept>**: <spread across N modules, should be consolidated>
2. ...

### Improvement Requirements Created
| SPEC-ID | Title | Priority | Effort |
|---------|-------|----------|--------|
| <ID> | <title> | <priority> | <effort> |

### Recommendations
1. <most impactful improvement to tackle first>
2. <second priority>
3. <third priority>
```

## Anti-Patterns to Avoid

- **Refactoring during review**: This skill diagnoses. Do not start refactoring mid-review. Create requirements and implement them separately.
- **Perfection bias**: Not every large file is a god object. Not every dependency is bad. Look for genuine pain, not theoretical violations.
- **Ignoring context**: A 2000-line file might be fine if it is auto-generated or contains a single complex algorithm. Read before judging.
- **Missing the forest**: Do not get lost in individual file metrics. The big questions are about module boundaries, domain alignment, and change propagation.
- **No prioritization**: A flat list of 50 issues is useless. Rank by impact on development velocity and defect rate.

## CLI Reference

```bash
aida list                              # Load all requirements for domain analysis
aida show <SPEC-ID>                    # Read specific requirement
aida search "<keyword>"                # Find requirements related to a module
aida add --title "..." --type task     # Create improvement requirement
aida comment add <SPEC-ID> "..."       # Record findings
aida rel add --from <ID> --to <ID> --type References  # Link to related requirements
```
