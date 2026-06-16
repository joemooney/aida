---
name: aida-test
description: Generate and run tests linked to requirements. Use when user wants to create tests for a requirement or verify requirement implementation.
allowed-tools:
  - Bash
  - Read
  - Edit
  - Write
  - Glob
  - Grep
---
<!-- AIDA Generated: v2.0.0 | checksum:dba13629 | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


# AIDA Test Generation Skill

## Purpose

Generate tests that verify requirement implementation, creating bidirectional traceability between tests and requirements.

## When to Use

Use this skill when:
- User wants to generate tests for a requirement
- User asks to "test", "verify", or "validate" a requirement
- User wants to check if a requirement's implementation is correct
- After implementing a requirement, to create verification tests

## Requirement Context

!`aida list --status completed 2>/dev/null | head -10 || echo "none"`

## Workflow

### Step 1: Load Requirement

```bash
aida show <SPEC-ID>
```

Display requirement details and identify testable behaviors from the description and acceptance criteria.

### Step 2: Identify Test Strategy

Based on the requirement type, determine appropriate test approach:
- **Functional**: Unit tests + integration tests for specific behaviors
- **Non-Functional**: Performance benchmarks, load tests
- **User Story**: End-to-end or acceptance tests
- **Bug Fix**: Regression test reproducing the original issue

### Step 3: Find Existing Implementation

Search for code implementing this requirement:

```bash
# Find trace comments linking to this requirement
grep -r "trace:<SPEC-ID>" --include="*.rs" --include="*.py" --include="*.ts" --include="*.js" src/ 2>/dev/null
```

### Step 4: Generate Tests

Create test files with trace comments linking back to the requirement:

```rust
// trace:<SPEC-ID> - Test: <requirement title> | ai:claude
#[test]
fn test_requirement_behavior() {
    // Test implementation
}
```

Guidelines:
- One test function per testable behavior
- Include both positive and negative test cases
- Add trace comments to every test function
- Follow project's existing test patterns and conventions

### Step 5: Run Tests

```bash
# Rust
cargo test --lib -- <test_name> 2>&1

# Python
python -m pytest <test_file> -v 2>&1

# TypeScript/JavaScript
npm test -- --grep "<test_name>" 2>&1
```

### Step 6: Link Test to Requirement

Create a `Verifies` relationship:

```bash
# If test requirement exists
aida rel add --from <TEST-SPEC-ID> --to <SPEC-ID> --type Verifies
```

Or add a comment documenting the test:

```bash
aida comment add <SPEC-ID> "Tests: <test_file>::<test_function>"
```

### Step 7: Report Results

Present to user:
```
## Test Results: <SPEC-ID>

### Tests Created
- test_behavior_one: PASS
- test_behavior_two: PASS
- test_edge_case: FAIL (reason)

### Coverage
- Testable behaviors identified: N
- Tests created: M
- Tests passing: P
```

## CLI Reference

```bash
aida show <SPEC-ID>                              # Show requirement
aida rel add --from <A> --to <B> --type Verifies # Link test to requirement
aida comment add <SPEC-ID> "Tests: ..."          # Document tests
aida edit <SPEC-ID> --status completed            # Mark verified
```

## Best Practices

- Name tests to reflect the requirement behavior, not implementation details
- Include trace comments in every test function for bidirectional traceability
- Test edge cases and error conditions, not just the happy path
- Run tests before marking requirement as verified