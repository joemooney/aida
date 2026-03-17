---
name: aida-code-review
description: Exhaustive code quality review — finds sloppy, untested, untraced, overly complex, and inconsistent code. Produces a structured report with before/after diffs.
allowed-tools:
  - Bash
  - Read
  - Glob
  - Grep
  - Write
  - Agent
---

# AIDA Code Review Skill

## Purpose

Perform an exhaustive quality review of project code. Go beyond what compilers and linters catch — find architectural issues, missing traceability, dead code, complexity hotspots, and inconsistencies that erode quality over time.

Produce a structured report with specific findings, severity levels, and proposed fixes.

## When to Use

- Before a release — ensure code quality meets the bar
- After a large feature push — catch technical debt introduced
- Periodically (monthly) — prevent code rot
- When onboarding — understand the codebase's health
- When someone says "this code is a mess"

## Review Dimensions

### 1. Compiler & Linter Issues (Automated First Pass)

Run all available automated tools before the manual review:

```bash
# Rust projects
cargo clippy --all-targets --all-features -- -W clippy::all -W clippy::pedantic 2>&1 | head -100
cargo test 2>&1 | tail -10
cargo audit 2>&1                    # known vulnerabilities
cargo outdated -R 2>&1 | head -20   # outdated dependencies

# Check for unused dependencies (if cargo-machete installed)
cargo machete 2>&1 || echo "install: cargo install cargo-machete"

# Lines of code per file (complexity indicator)
tokei --sort code 2>&1 | head -20 || find . -name "*.rs" -exec wc -l {} \; | sort -rn | head -20
```

Record all warnings and errors — these are the baseline issues.

### 2. Traceability Coverage

Every significant function should trace back to a requirement. Check:

```bash
# Find all trace comments
grep -rn "trace:" --include="*.rs" src/ | head -20

# Find files with NO trace comments (potential gaps)
find src/ -name "*.rs" -exec sh -c 'grep -L "trace:" "$1" 2>/dev/null' _ {} \;

# Cross-reference: do traced spec IDs exist in the database?
grep -oP 'trace:\K[A-Z]+-[0-9]+' --include="*.rs" -r src/ | sort -u | while read id; do
  aida show "$id" >/dev/null 2>&1 || echo "ORPHAN TRACE: $id (not in database)"
done
```

**What to flag:**
- Source files >100 lines with no trace comments
- Trace comments referencing deleted/rejected requirements
- Features with no test coverage linked to requirements
- Test files with no `// trace:` linking them to what they verify

### 3. Complexity Analysis

**File-level complexity:**
- Files over 500 lines → should be split
- Files over 1000 lines → must be split (god objects)
- Functions over 50 lines → too complex, extract helpers
- Functions over 100 lines → critical: refactor immediately
- Nesting depth > 4 levels → flatten with early returns or extraction

```bash
# Find long files
find src/ -name "*.rs" -exec wc -l {} \; | sort -rn | head -20

# Find long functions (approximate: count lines between fn and closing brace)
grep -n "pub fn \|fn " src/**/*.rs 2>/dev/null | head -30
```

**Cognitive complexity indicators:**
- Multiple nested `match` statements
- Chains of `.and_then().map().unwrap_or_else()`
- Boolean parameters that change function behavior (use enums instead)
- Functions doing two things (create AND validate, fetch AND transform)

### 4. Dead Code & Unused Items

```bash
# Compiler dead code warnings
cargo build 2>&1 | grep "dead_code\|unused"

# Unused dependencies
cargo machete 2>&1

# Look for TODO/FIXME/HACK/TEMP comments
grep -rn "TODO\|FIXME\|HACK\|TEMP\|XXX\|DEPRECATED" --include="*.rs" src/

# Functions that are pub but never called from outside their module
# (approximate — check for low-usage public APIs)
```

### 5. Error Handling Quality

- `unwrap()` calls in non-test code → should use `?` or `.expect("reason")`
- `expect("")` with empty messages → should explain what went wrong
- Bare `Err(e)` without context → use `.context("doing what")` from anyhow
- Swallowed errors (`let _ = might_fail()`) → should at least log

```bash
# Count unwrap() calls in non-test code
grep -rn "\.unwrap()" --include="*.rs" src/ | grep -v "_test\|#\[test\]\|tests/" | wc -l

# Find bare unwrap without explanation
grep -rn "\.unwrap()" --include="*.rs" src/ | grep -v "_test\|#\[test\]\|tests/" | head -20
```

### 6. Consistency

- Naming conventions: `snake_case` functions, `CamelCase` types, `SCREAMING_SNAKE` constants
- Similar functions implemented differently in different modules
- Inconsistent error handling patterns (some use `anyhow`, some use custom errors)
- Mixed async patterns (some functions block, some are async for same operation)
- Import style varies (`use` at top vs inline `std::path::Path`)

### 7. Security

- Hardcoded secrets, API keys, tokens, passwords
- SQL injection (raw string interpolation in queries)
- Path traversal (user input used in file paths without sanitization)
- Unsafe code without justification comments
- Dependencies with known vulnerabilities (`cargo audit`)

```bash
# Check for potential secrets
grep -rn "password\|secret\|api_key\|token" --include="*.rs" src/ | grep -v "test\|example\|doc\|comment"

# Unsafe code blocks
grep -rn "unsafe " --include="*.rs" src/

# cargo audit
cargo audit 2>&1
```

### 8. Test Quality

- Test coverage: are the critical paths tested?
- Test names: do they describe behavior, not implementation?
- Test isolation: do tests depend on external state (network, filesystem)?
- Flaky tests: do any tests fail intermittently?
- Missing edge cases: empty input, max values, concurrent access

```bash
# Count tests
cargo test -- --list 2>&1 | grep "test$" | wc -l

# Run tests and check for failures
cargo test 2>&1 | tail -5

# Find modules without tests
find src/ -name "*.rs" | while read f; do
  grep -q "#\[cfg(test)\]\|#\[test\]" "$f" || echo "NO TESTS: $f"
done
```

### 9. Documentation Quality (Code-Level)

- Public APIs without doc comments (`/// ...`)
- Doc comments that just repeat the function name ("Creates a new Foo")
- Missing `# Examples` in doc comments for complex APIs
- Stale doc comments that don't match the function signature
- `#[allow(missing_docs)]` suppressing documentation requirements

### 10. Dependency Health

```bash
# Outdated dependencies
cargo outdated -R 2>&1 | head -20

# Security vulnerabilities
cargo audit 2>&1

# Dependency tree depth (deep trees = fragile)
cargo tree --depth 1 2>&1 | wc -l

# License compatibility
cargo deny check licenses 2>&1 || echo "install: cargo install cargo-deny"
```

#### cargo-deny Integration

`cargo-deny` checks 4 dimensions that `cargo audit` alone misses:

```bash
# Full check (licenses + bans + advisories + sources)
cargo deny check 2>&1 | head -30

# Just license compatibility
cargo deny check licenses 2>&1

# Just security advisories (superset of cargo-audit)
cargo deny check advisories 2>&1

# Check for banned crates
cargo deny check bans 2>&1
```

If `cargo-deny` is not installed:
```bash
cargo install cargo-deny
```

Create a default `deny.toml` if one doesn't exist:
```bash
cargo deny init
```

## Workflow

### Step 1: Automated Scan

Run all automated tools first and collect their output:

```bash
# Create a temporary report directory
mkdir -p .aida/review

# Run all checks
cargo clippy --all-targets 2>&1 > .aida/review/clippy.txt
cargo test 2>&1 > .aida/review/tests.txt
cargo audit 2>&1 > .aida/review/audit.txt
cargo outdated -R 2>&1 > .aida/review/outdated.txt
```

### Step 2: Manual Review (AI-Assisted)

For each source file, check dimensions 2-9 above. Focus on:
1. Files changed recently (most likely to have issues)
2. Large files (most likely to be complex)
3. Files without tests or trace comments

### Step 3: Categorize Findings

For each issue found, record:
- **File** and **line number**
- **Severity**: Critical / Important / Minor
- **Category**: Traceability / Complexity / Dead Code / Error Handling / Consistency / Security / Tests / Docs
- **Description**: What's wrong
- **Before/After**: Proposed fix (if applicable)

### Step 4: Generate Report

Output a structured report in two formats:

**Markdown** (`docs/code-review-report.md`):
```markdown
# Code Review Report
Generated: YYYY-MM-DD
Files reviewed: N | Issues: N

## Summary
| Category | Critical | Important | Minor |
|----------|----------|-----------|-------|
| Traceability | 5 | 12 | 3 |
| Complexity | 2 | 8 | 15 |
...

## Findings
### src/models.rs
**CRITICAL: Function too long (342 lines)**
- `generate_requirement_id()` at line 4323
- Split into: prefix resolution, number generation, formatting
...
```

**HTML** (`docs/code-review-report.html`):
- Same dark-theme side-by-side diff viewer as /aida-docs-review
- Filterable by severity and category
- Collapsible file sections

**SARIF** (optional, for GitHub integration):
```json
{
  "$schema": "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/main/sarif-2.1/schema/sarif-schema-2.1.0.json",
  "version": "2.1.0",
  "runs": [{
    "tool": { "driver": { "name": "aida-code-review" } },
    "results": [...]
  }]
}
```

### Step 5: Integration Options

**GitHub PR Review**: Upload SARIF to GitHub Code Scanning:
```bash
# If running in GitHub Actions:
gh api repos/{owner}/{repo}/code-scanning/sarifs -f sarif=@code-review.sarif
```

**Codex Review**: Feed findings to Codex for automated fixing:
```bash
codex exec "Read .aida/review/report.md. Fix all CRITICAL issues. Show each fix before applying."
```

**AIDA Requirements**: Create requirements for major findings:
```bash
aida add --title "Refactor: split models.rs (1000+ lines)" --type task --status draft
aida add --title "Fix: add error context to 15 bare unwrap() calls" --type task --status draft
```

### Step 6: Apply Fixes (Optional)

Ask the user:
- **Auto-fix clippy warnings?** `cargo clippy --fix`
- **Create tasks for manual fixes?** Generate AIDA requirements
- **Apply specific fixes?** Show diff for each, get approval

## Diff-Aware Mode

When reviewing only changed files (e.g., before a PR), use diff-aware mode:

### Step 0: Identify Changed Files

```bash
# Changes since branching from main
git diff --name-only main...HEAD -- '*.rs'

# Changes in the last commit
git diff --name-only HEAD~1

# Staged changes only
git diff --cached --name-only -- '*.rs'
```

Only review files returned by these commands. This dramatically reduces review time for incremental changes while still catching issues in new/modified code.

### Comparing Against Baseline

If a previous review report exists, compare:

```bash
# Previous review
cat docs/code-review-report.md | grep "^##\\|CRITICAL\\|IMPORTANT" > /tmp/baseline.txt

# Current review
# (run full review, then compare)
diff /tmp/baseline.txt /tmp/current.txt
```

Report new issues vs resolved issues since last review.

## Severity Guidelines

| Level | Criteria | Examples |
|-------|----------|---------|
| **Critical** | Will cause bugs or security issues | `unwrap()` on user input, SQL injection, hardcoded secrets |
| **Important** | Makes code hard to maintain | Functions >100 lines, no tests for critical path, dead code |
| **Minor** | Style or convention issues | Missing doc comments, naming inconsistency, unused imports |

## Anti-Patterns to Flag

### The God File
One file with 1000+ lines doing everything. Split by responsibility.

### The Unwrap Forest
Production code littered with `.unwrap()`. Use `?`, `.context()`, or `.expect("reason")`.

### The Orphan Function
Public function with no callers. Either it's dead code or it's missing a trace link.

### The Untested Happy Path
Code that only tests the success case. Add tests for: empty input, invalid input, boundary values, concurrent access.

### The Stringly Typed API
Functions taking `&str` where an enum would be safer. `status: &str` → `status: RequirementStatus`.

### The Clone Storm
Excessive `.clone()` calls suggesting ownership issues. Consider references or `Cow<'_, str>`.

### The Comment Novel
Functions with more comment lines than code lines. The code should be self-explanatory; refactor instead of commenting.

## CLI Reference

```bash
# Automated tools
cargo clippy --all-targets --all-features
cargo test
cargo audit
cargo outdated -R
cargo machete

# AIDA traceability
grep -rn "trace:" --include="*.rs" src/
aida show <SPEC-ID>

# Code metrics
find src/ -name "*.rs" -exec wc -l {} \; | sort -rn | head -20
grep -rn "\.unwrap()" --include="*.rs" src/ | grep -v test | wc -l
grep -rn "TODO\|FIXME\|HACK" --include="*.rs" src/
```
