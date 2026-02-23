---
name: aida-compiler-warnings
description: Analyze compiler warnings across the workspace, categorize by risk level, and recommend a prioritized action plan. Use when the user wants to clean up warnings or assess code health.
allowed-tools:
  - Bash
  - Read
  - Grep
  - Glob
  - Edit
---

# AIDA Warnings Analysis Skill

## Purpose

Run compiler/linter diagnostics across the Rust workspace, parse and categorize all warnings, and produce a prioritized action plan grouped by risk level.

## When to Use

Use this skill when:
- User says "check warnings", "analyze warnings", "clean up warnings", or "lint"
- User wants to assess code health or warning debt
- After a large refactor to verify no new issues were introduced
- Periodic maintenance to prevent warning accumulation

## Workflow

### Step 1: Collect Warnings

Run clippy (preferred) or cargo build to collect all warnings:

```bash
cargo clippy --workspace --all-targets 2>&1 | grep "^warning" || cargo build --workspace 2>&1 | grep "^warning"
```

Also capture the full output for detailed analysis:

```bash
cargo clippy --workspace --all-targets 2>&1
```

### Step 2: Parse and Categorize

Group every warning into one of these risk categories:

#### Safe Auto-Fix (Risk: None)
Warnings that `cargo fix` or `cargo clippy --fix` can resolve automatically:
- `unused_imports` — unused `use` statements
- `unused_mut` — variables that don't need `mut`
- `unused_variables` — variables that should be prefixed with `_`

**Recommended action:** Run `cargo fix --workspace --allow-dirty` or `cargo clippy --fix --workspace --allow-dirty`

#### Low Risk (Risk: Low)
Warnings that require manual removal but are straightforward:
- `dead_code` — unused functions, methods, constants, enum variants
- `unreachable_code` — code after return/break/continue

**Recommended action:** Verify the code isn't used via feature flags, conditional compilation, or external crates, then remove. Check with `grep` before deleting.

#### Medium Risk (Risk: Medium)
Warnings that need careful review before addressing:
- `unused_assignments` — value assigned but never read (may indicate a logic bug)
- `unexpected_cfgs` — invalid `cfg` conditions (may indicate a missing feature flag)
- Fields never read on structs (may be part of a public API or serialization)
- Deprecated API usage

**Recommended action:** Review each instance individually. These may indicate real bugs or missing implementations.

#### Review Needed (Risk: Varies)
Warnings that might indicate actual bugs:
- `unused_results` — Result/Option not checked
- `unreachable_patterns` — match arms that can never match
- Type mismatches or truncation warnings
- Any clippy lint categorized as `correctness`

**Recommended action:** Investigate each one. These are the highest-value warnings to fix.

### Step 3: Generate Report

Present the findings as a structured report:

```
## Warnings Analysis Report

**Total warnings:** N across M crates
**Breakdown by crate:** aida-core (X), aida-desktop (Y), aida-server (Z), ...

### Safe Auto-Fix (N warnings)
These can be fixed automatically with `cargo fix` or `cargo clippy --fix`:
- unused_imports: N (files: ...)
- unused_mut: N (files: ...)
- unused_variables: N (files: ...)

### Low Risk — Dead Code Removal (N warnings)
Unused code that can likely be removed after verification:
- unused functions: N
- unused enum variants: N
- unused struct fields: N
- unused constants: N

### Medium Risk — Needs Review (N warnings)
- unused_assignments: N (may indicate logic bugs)
- unexpected_cfgs: N (may indicate missing features)
- never-read fields: N

### Review Needed (N warnings)
- [list each with file:line and explanation]

## Recommended Action Plan

1. **Quick win (< 5 min):** Run `cargo fix --workspace --allow-dirty` for auto-fixable warnings
2. **Easy cleanup (15-30 min):** Remove dead code in Low Risk category after grep verification
3. **Careful review (30-60 min):** Investigate Medium Risk items one by one
4. **Bug investigation:** Review high-risk warnings for potential bugs
```

### Step 4: Optionally Fix

If the user asks to fix warnings:
- **Safe auto-fix only:** Run `cargo fix` / `cargo clippy --fix`
- **Low risk:** Remove dead code after verifying with grep that nothing references it
- **Medium/High risk:** Present each warning and ask user before making changes

## Important Notes

- Never auto-fix medium or high risk warnings without user confirmation
- Be aware of conditional compilation (`#[cfg(...)]`) — code that looks dead may be used under different feature flags or target architectures (especially `wasm32`)
- Check if "unused" items are part of a public API that external crates depend on
- When removing dead code, check if it's referenced in tests (`--all-targets` may show different results than `--lib`)
- After fixes, re-run `cargo build --workspace` to confirm no regressions
