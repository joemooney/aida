# Code Review Skill Research Report

**Date:** 2026-03-17
**Purpose:** Research best practices for an automated code quality review skill for Rust projects, covering tools, metrics, checklists, traceability, and integration formats.

## Related Requirements
- Existing skill: `/aida-code-review` (`aida-core/templates/skills/aida-code-review.md`)
- Existing skill: `/aida-compiler-warnings` (`aida-core/templates/skills/aida-compiler-warnings.md`)
- Existing skill: `/aida-review` (requirements-focused review)

## Status
Research Complete

---

## 1. Rust-Specific Code Quality Tools

### 1.1 Clippy (Built-in Linter)

Clippy is Rust's official linter with **800+ lints** organized into 9 categories:

| Category | Default | What It Catches |
|----------|---------|-----------------|
| `clippy::correctness` | **deny** | Outright wrong or useless code |
| `clippy::suspicious` | warn | Most likely wrong or useless code |
| `clippy::style` | warn | Non-idiomatic code patterns |
| `clippy::complexity` | warn | Unnecessarily complex implementations |
| `clippy::perf` | warn | Code that could run faster |
| `clippy::pedantic` | allow | Stricter lints with occasional false positives |
| `clippy::restriction` | allow | Lints that prevent certain language/library features |
| `clippy::nursery` | allow | Experimental lints under development |
| `clippy::cargo` | allow | Cargo manifest issues |

**Key insight for the review skill:** The default `cargo clippy` only runs correctness/suspicious/style/complexity/perf. The `pedantic`, `restriction`, and `nursery` categories contain high-value lints that most projects never enable:

- **pedantic**: `cast_lossless` (unsafe numeric casts), `cloned_instead_of_copied` (iterator inefficiency)
- **restriction**: `arithmetic_side_effects` (potential overflow/panics), `as_conversions` (silent data loss), `allow_attributes_without_reason` (undocumented lint suppression)
- **nursery**: `collection_is_never_read` (dead collections), `branches_sharing_code` (duplicated branches)

**Recommendation for skill:** Run `cargo clippy -- -W clippy::pedantic -W clippy::nursery` in addition to default lints. Selectively enable restriction lints like `arithmetic_side_effects` and `as_conversions` for safety-critical code.

### 1.2 cargo-audit (Security Vulnerabilities)

- **What:** Audits `Cargo.lock` against the RustSec Advisory Database for known CVEs
- **Value:** High -- catches dependencies with published security advisories
- **Install:** `cargo install cargo-audit`
- **Usage:** `cargo audit`
- **Output:** Lists vulnerable crates with advisory IDs, severity, affected versions, and patched versions
- **Integration:** Can produce JSON output for CI/CD consumption

### 1.3 cargo-deny (Dependency Policy Engine)

Four distinct checks in one tool:

1. **Licenses** -- Verifies all dependency licenses are acceptable (critical for commercial/regulated projects)
2. **Bans** -- Denies/allows specific crates; detects duplicate versions of the same crate
3. **Advisories** -- Vulnerability scanning (overlaps with cargo-audit but configurable)
4. **Sources** -- Ensures crates come only from trusted registries

- **Config:** TOML-based (`deny.toml`)
- **Install:** `cargo install --locked cargo-deny`
- **Usage:** `cargo deny check` (all) or `cargo deny check licenses` (specific)
- **CI:** GitHub Action available (`cargo-deny-action`)
- **Value:** Very high -- the only tool that covers license compliance and source trust

### 1.4 cargo-machete (Unused Dependencies -- Fast)

- **What:** Detects unused dependencies by scanning source files for import references
- **Method:** Text-based scanning (fast but imprecise -- may produce false positives for build-script-only or code-generated deps)
- **Install:** `cargo install cargo-machete`
- **Usage:** `cargo machete` or `cargo machete /path`
- **Exit codes:** 0 = clean, 1 = unused found, 2 = error
- **Config:** `Cargo.toml` metadata section for `ignored` dependencies and `renamed` crate mappings
- **Accuracy boost:** `--with-metadata` flag uses `cargo metadata --all-features` for better precision
- **Value:** High -- fast enough for CI, catches dependency bloat

### 1.5 cargo-udeps (Unused Dependencies -- Precise)

- **What:** Same goal as cargo-machete but uses compilation analysis rather than text scanning
- **Method:** Creates temporary workspace, runs `cargo update`, compares dependency trees
- **Requirement:** Needs Rust **nightly** toolchain
- **Install:** `cargo install cargo-udeps --locked`
- **Usage:** `cargo +nightly udeps`
- **Limitations:** Some deps used by stdlib may go undetected; tracks by name only (multiple versions of same crate can cause issues)
- **Value:** Medium -- more precise than machete but nightly requirement is a barrier

### 1.6 cargo-geiger (Unsafe Code Audit)

- **What:** Counts and maps `unsafe` code usage across the entire dependency tree
- **Output:** Statistical report showing unsafe code distribution per crate
- **Install:** `cargo install --locked cargo-geiger`
- **Usage:** `cargo geiger`
- **Purpose:** "Statistical input to auditing" -- not a security verdict, but a risk indicator
- **Complements:** cargo-crev (code review), safety-dance (community effort to reduce unsafe)
- **Value:** Medium-high for safety-critical projects; helps identify risk in dependency tree

### 1.7 cargo-outdated (Dependency Freshness)

- **What:** Shows which dependencies have newer versions available
- **Method:** Creates temporary workspace, runs `cargo update`, compares original vs updated dependency tree
- **Output:** Table with columns: Name, Project (current), Compat (compatible update), Latest (newest), Kind, Platform
- **Install:** `cargo install --locked cargo-outdated`
- **Usage:** `cargo outdated -R` (root deps only)
- **Flags:** `-d NUM` (depth), `--format json` (machine-readable), `--exit-code 1` (fail if outdated)
- **Value:** Medium -- stale deps accumulate security risk and compatibility debt

### 1.8 cargo-bloat (Binary Size Analysis)

- **What:** Identifies which functions and crates contribute most to binary size
- **Output:** Table with File%, .text%, Size, Crate, Name
- **Modes:** Function-level (default) or crate-level (`--crates`)
- **Supports:** ELF, Mach-O, PE (not WASM -- use `twiggy` for that)
- **Install:** `cargo install cargo-bloat`
- **Value:** Low-medium -- useful for embedded/WASM targets where binary size matters

### 1.9 tokei (Lines of Code)

- **What:** Fast, accurate line counter for 150+ languages
- **Metrics:** Files, Total Lines, Code Lines, Comment Lines, Blank Lines
- **Output:** Terminal table (default), JSON, YAML, CBOR
- **Features:** Handles multi-line comments, nested comments, strings correctly; respects `.gitignore`
- **Install:** `cargo install tokei`
- **Value:** Medium -- useful as a complexity indicator and to track codebase growth

### 1.10 rust-code-analysis (Mozilla, Code Metrics)

- **What:** Library + CLI for computing 11 maintainability metrics across Rust (and 9 other languages)
- **Built on:** Tree-sitter parsing
- **Metrics computed:**
  - **CC** -- Cyclomatic Complexity (control flow paths)
  - **Cognitive Complexity** -- human comprehension difficulty
  - **SLOC/PLOC/LLOC** -- Source/Physical/Logical lines of code
  - **CLOC** -- Comment lines
  - **BLANK** -- Blank lines
  - **Halstead** -- Effort, volume, difficulty, vocabulary metrics
  - **MI** -- Maintainability Index (composite score)
  - **NOM** -- Number of methods/functions per unit
  - **NEXITS** -- Exit points per function
  - **NARGS** -- Arguments per function
- **Install:** `cargo install rust-code-analysis-cli`
- **License:** MPL-2.0
- **Value:** High -- the only Rust-native tool that computes cognitive complexity and Halstead metrics

### 1.11 CodeQL (GitHub Advanced Security)

- **What:** Semantic code analysis engine (queries code as data)
- **Rust support:** Yes (added to CodeQL's supported languages)
- **Capabilities:** Security vulnerability detection, taint analysis, data flow tracking
- **Integration:** Native to GitHub Advanced Security; SARIF output
- **Value:** High for security-focused review, but requires GitHub Advanced Security license for private repos

### Tool Priority Matrix

| Tool | Value | Speed | Install Friction | Recommended |
|------|-------|-------|-----------------|-------------|
| clippy (pedantic+nursery) | Very High | Fast | None (built-in) | Always |
| cargo-audit | High | Fast | Low | Always |
| cargo-deny | Very High | Medium | Low | Always |
| cargo-machete | High | Fast | Low | Always |
| cargo-outdated | Medium | Slow | Low | Periodic |
| cargo-geiger | Medium-High | Medium | Low | Safety-critical |
| cargo-bloat | Low-Medium | Medium | Low | Binary size concerns |
| tokei | Medium | Very Fast | Low | Always (metrics baseline) |
| rust-code-analysis | High | Medium | Medium | Complexity analysis |
| cargo-udeps | Medium | Slow | High (nightly) | When machete insufficient |

---

## 2. Code Complexity Metrics

### 2.1 Cyclomatic Complexity (CC)

- **Definition:** Number of linearly independent paths through a function's control flow graph
- **Calculation:** CC = E - N + 2P (edges minus nodes plus 2 times connected components); or simply count decision points (if, else, while, for, match arm, &&, ||) + 1
- **Thresholds (widely adopted):**
  - 1-10: Simple, low risk
  - 11-20: Moderate complexity, moderate risk
  - 21-50: Complex, high risk
  - 51+: Untestable, very high risk
- **Limitation:** Treats all branches equally; a flat switch/match with 20 cases scores the same as deeply nested conditionals, despite being far easier to understand

### 2.2 Cognitive Complexity (SonarSource)

- **Definition:** Measures how difficult code is for humans to *understand* (vs. testability)
- **Key difference from CC:** Penalizes nesting depth; a nested `if` inside a `for` inside a `match` scores much higher than flat sequential branches
- **Calculation principles:**
  - Increments for each break in linear flow (if, for, while, catch, etc.)
  - Additional increment for each level of nesting
  - No increment for shorthand structures (ternary, null coalescing)
  - Boolean operator sequences count once per sequence, not per operator
- **Thresholds (SonarSource defaults):**
  - 0-15: Good
  - 16-25: Needs attention
  - 26+: Should be refactored
- **Tool support:** `rust-code-analysis` computes cognitive complexity for Rust
- **Recommendation:** Use cognitive complexity as the primary metric for the review skill, as it better reflects human comprehension difficulty

### 2.3 Halstead Metrics

Suite of metrics based on counting operators and operands:
- **Vocabulary** (n) = n1 + n2 (unique operators + unique operands)
- **Length** (N) = N1 + N2 (total operators + total operands)
- **Volume** (V) = N * log2(n) -- program size in bits
- **Difficulty** (D) = (n1/2) * (N2/n2) -- proneness to errors
- **Effort** (E) = D * V -- cognitive effort to understand
- **Time** (T) = E / 18 -- estimated time to understand in seconds
- **Bugs** (B) = V / 3000 -- estimated bugs

**Value for review skill:** Halstead effort and difficulty can flag functions that are mathematically dense even if structurally simple (lots of distinct operations on many variables).

### 2.4 Maintainability Index (MI)

- **Composite metric:** Combines Halstead Volume, Cyclomatic Complexity, and Lines of Code
- **Formula:** MI = 171 - 5.2 * ln(V) - 0.23 * CC - 16.2 * ln(LOC)
- **Scale:** 0-100 (higher is better); some tools normalize to 0-100
- **Thresholds:** 0-9 = low maintainability, 10-19 = moderate, 20+ = high
- **Value:** Single number for quick triage but can be misleading in isolation

### 2.5 Practical Thresholds for the Review Skill

| Metric | Good | Warning | Critical |
|--------|------|---------|----------|
| File lines | <500 | 500-1000 | >1000 |
| Function lines | <50 | 50-100 | >100 |
| Cyclomatic complexity | <10 | 10-20 | >20 |
| Cognitive complexity | <15 | 15-25 | >25 |
| Nesting depth | <=3 | 4 | >4 |
| Function arguments | <=5 | 6-7 | >7 |
| Exit points per function | <=3 | 4-5 | >5 |

---

## 3. Code Review Best Practices & Checklists

### 3.1 Google's Engineering Practices

Google's code review guidelines (publicly available) identify 8 key areas:

1. **Design** -- Does the code fit the system architecture? Is this the right time for this change?
2. **Functionality** -- Does it do what the developer intended? Does it benefit end-users AND future developers? Edge cases, concurrency issues?
3. **Complexity** -- "Can't be understood quickly by code readers." Guard against over-engineering.
4. **Tests** -- Correct, well-designed automated tests. Tests are also code that must be maintained.
5. **Naming** -- Communicates purpose without excessive length.
6. **Comments** -- Explain *why*, not *what*. Code clarity should eliminate unnecessary commentary.
7. **Style** -- Compliance with style guides.
8. **Documentation** -- Update READMEs and docs when functionality changes.

**Key principle:** "Reviewers should favor approving a CL once it is in a state where it definitely improves the overall code health of the system" -- even if imperfect. Use "Nit:" prefix for non-critical suggestions.

### 3.2 The 8 Pillars of Quality Engineering (DocuWriter)

1. **Documentation Quality** -- Public APIs documented? Doc comments match signatures?
2. **Style Consistency** -- Naming conventions, formatting, import style uniform?
3. **Logic & Correctness** -- Trace execution paths mentally. Boundary conditions? Edge cases?
4. **Test Coverage** -- Meaningful assertions (not just execution)? Error case tests?
5. **Security Implications** -- OWASP Top 10 as a baseline. Input validation? Auth checks?
6. **Performance Impact** -- Big O analysis. N+1 queries? Missing indexes? Caching?
7. **Code Duplication (DRY)** -- Exact matches AND similar logic patterns. Shared utility functions?
8. **Dependency Management** -- License compatibility? Maintenance status? API SLAs?

### 3.3 What Experienced Reviewers Catch That Tools Miss

This is the core value proposition for an AI-powered review skill:

- **Architectural fit** -- Does this change belong here? Does it follow the patterns established elsewhere in the codebase?
- **Intent mismatch** -- Code compiles and passes tests but doesn't actually solve the stated problem
- **Missing abstraction** -- Three places doing similar things that should share a function
- **Temporal coupling** -- Functions that must be called in a specific order but nothing enforces it
- **State management issues** -- Mutable state shared across threads without synchronization
- **API design problems** -- Public API that paints you into a corner for future changes
- **Implicit assumptions** -- Code that works only because of an unstated invariant
- **Error swallowing** -- `let _ = might_fail()` or `if let Ok(v) = ...` that silently ignores errors
- **Type system underuse** -- Using strings where enums would be safer ("stringly typed")
- **Ownership anti-patterns** -- Excessive `.clone()` calls suggesting misunderstood ownership

### 3.4 Rust-Specific Review Patterns

Beyond general best practices, Rust code reviews should check:

- **unwrap() in non-test code** -- Should use `?`, `.expect("reason")`, or `.context()`
- **Clone storms** -- Excessive `.clone()` suggesting ownership issues; consider `Cow<'_, T>` or references
- **Stringly-typed APIs** -- `fn process(status: &str)` should be `fn process(status: Status)`
- **Boolean parameters** -- `fn render(is_admin: bool, show_header: bool)` should use enums or builder pattern
- **Unsafe blocks** -- Must have `// SAFETY:` comment explaining why it's sound
- **async/sync mixing** -- Blocking calls inside async functions (use `spawn_blocking`)
- **Panic paths** -- `panic!()`, `todo!()`, `unimplemented!()` in production paths
- **Lifetime annotations** -- Unnecessary lifetime parameters that could be elided
- **Error type design** -- Custom error types vs anyhow for libraries vs applications

---

## 4. Traceability & Requirements Coverage

### 4.1 Industry Standards

Three major standards require formal traceability:

**DO-178C (Aerospace)**
- Mandates traceability across: System Requirements -> High-Level Software Requirements -> Low-Level Requirements -> Source Code -> Tests
- "If there are architectural elements or source code that can't be traced to a requirement, then it's a risk and shouldn't be there"
- Structural coverage analysis: testing must exercise actual code paths, not just validate against requirements
- Bidirectional: forward (req -> code) AND backward (code -> req)

**ISO 26262 (Automotive)**
- ASIL D (highest risk): Bidirectional traceability spanning hazard analysis, safety goals, functional requirements, technical requirements, implementation, and verification
- "Every safety requirement must trace forward to its implementation and backward to the hazard it addresses"

**ASPICE (Automotive Process)**
- Level 2: Bidirectional traceability required
- Level 3: Must follow a standardized, repeatable organizational process

### 4.2 Traceability Directions

- **Forward traceability:** Requirements -> Design -> Code -> Tests (ensures every requirement is implemented and tested)
- **Backward traceability:** Code -> Requirements (ensures every line of code exists for a reason -- no dead code)
- **Bidirectional:** Both directions -- the gold standard; enables impact analysis when requirements change

### 4.3 Best Practices for Code Traceability

1. **Establish clear, measurable requirements** -- Vague requirements make traceability meaningless
2. **Implement bidirectional tracing** -- Forward (req -> code) catches unimplemented requirements; backward (code -> req) catches dead code
3. **Automate trace link creation and validation** -- Manual traceability matrices are error-prone and stale within days
4. **Audit trace links regularly** -- Check for orphan traces (code referencing deleted requirements), missing traces (code with no requirement), and stale traces (code changed but trace not updated)
5. **Change impact analysis** -- When a requirement changes, trace links show exactly what code and tests are affected

### 4.4 AIDA's Current Traceability Approach

AIDA uses inline trace comments:
```rust
// trace:FR-0042 | ai:claude
fn implement_feature() { ... }
```

**Current skill coverage:**
- `/aida-review` -- Checks trace comments against requirements database
- `/aida-code-review` -- Flags files without trace comments
- `/aida-commit` -- Validates trace comments at commit time

**Gaps the enhanced review skill should address:**
- Backward traceability: Find code without trace links (potential dead code or unspecified features)
- Forward traceability: Find requirements with no implementing code
- Orphan detection: Trace comments referencing deleted/rejected requirements
- Coverage completeness: Requirements marked "completed" but only partially implemented
- Change impact: When a file is modified, check if traced requirement status needs updating

---

## 5. Dead Code & Unused Dependency Detection

### 5.1 Rust Compiler's Built-in Dead Code Detection

The Rust compiler includes several warn-by-default lints for unused code:

| Lint | What It Detects |
|------|-----------------|
| `dead_code` | Unused unexported items (functions, structs, fields) |
| `unused_imports` | Unused `use` statements |
| `unused_variables` | Variable bindings never used |
| `unused_assignments` | Assignments overwritten before being read |
| `unused_macros` | Macro definitions never invoked |
| `unused_mut` | Unnecessary `mut` keyword |
| `unreachable_code` | Code after return/break/continue |
| `unreachable_patterns` | Match arms that can never match |
| `unused_labels` | Labels never used in break/continue |

All are part of the `#[warn(unused)]` lint group.

**Limitations:**
- Only detects unused items within the crate; `pub` items are assumed used externally
- Conditional compilation (`#[cfg(...)]`) can hide dead code from one target
- Feature-gated code may appear dead when built without that feature
- Binary crates: everything reachable from `main()` is considered live
- Library crates: everything `pub` in the root module is considered live

### 5.2 Beyond the Compiler

**cargo-machete** (fast, text-based):
- Scans source for references to declared dependencies
- Fast enough for CI pre-commit hooks
- False positives for build-script-only deps, proc macros, codegen'd imports

**cargo-udeps** (precise, compilation-based):
- Analyzes actual compilation to find unused deps
- Requires nightly Rust
- More precise but slower and harder to set up

**For the review skill, the recommended approach is:**
1. Run `cargo clippy` with all warnings enabled to catch compiler-detectable dead code
2. Run `cargo machete` for unused dependency detection
3. Use AI analysis to find "effectively dead" code: `pub` functions with zero callers within the project (that aren't part of a public library API)
4. Search for TODO/FIXME/HACK/TEMP markers that indicate provisional code

### 5.3 Detecting "Effectively Dead" Public Functions

The compiler can't detect public functions that are never called. The review skill should:

```bash
# Find all pub fn definitions
grep -rn "pub fn " --include="*.rs" src/

# For each, search for callers (excluding the definition itself)
# If zero callers found and it's not a trait implementation or external API,
# flag as potentially dead
```

This is where AI review adds value over static tools -- understanding whether a `pub fn` is part of the intended public API, a trait obligation, or genuinely dead.

---

## 6. Code Review Report Formats & Integration

### 6.1 SARIF (Static Analysis Results Interchange Format)

SARIF 2.1.0 is the industry standard for tool output. Key structure:

```json
{
  "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
  "version": "2.1.0",
  "runs": [{
    "tool": {
      "driver": {
        "name": "aida-code-review",
        "version": "1.0.0",
        "rules": [{
          "id": "AIDA-COMPLEXITY-001",
          "shortDescription": { "text": "Function exceeds complexity threshold" },
          "fullDescription": { "text": "..." },
          "help": { "text": "..." },
          "defaultConfiguration": { "level": "warning" },
          "properties": { "precision": "high" }
        }]
      }
    },
    "results": [{
      "ruleId": "AIDA-COMPLEXITY-001",
      "level": "warning",
      "message": { "text": "Function `process_requirement` has cognitive complexity 32 (threshold: 15)" },
      "locations": [{
        "physicalLocation": {
          "artifactLocation": { "uri": "src/models.rs" },
          "region": { "startLine": 142, "startColumn": 1, "endLine": 285, "endColumn": 1 }
        }
      }],
      "partialFingerprints": {
        "primaryLocationLineHash": "abc123..."
      }
    }]
  }]
}
```

**GitHub integration:**
- GitHub Code Scanning natively ingests SARIF via the `upload-sarif` action or REST API
- Results appear as annotations on PRs and in the Security tab
- Size limits: 10MB compressed, 25,000 results per run, 25,000 rules per run
- Fingerprinting (`partialFingerprints`) prevents duplicate alerts across runs

### 6.2 Reviewdog

Reviewdog is a tool-agnostic review comment poster that:
- Accepts linter output in multiple formats: errorformat (vim-style), RDFormat (JSON), Checkstyle XML, SARIF, diff
- Filters findings to only report NEW issues (diff-aware)
- Posts as GitHub PR Check annotations, PR review comments, or code suggestions
- Supports all major CI systems (GitHub Actions, Travis, Circle, GitLab, Jenkins)
- Can be configured via `.reviewdog.yml` to run multiple linters

**Value for AIDA:** If the skill produces SARIF or RDFormat output, reviewdog can post findings directly to GitHub PRs without custom integration code.

### 6.3 GitHub PR Review API

Direct PR comment posting via the REST API:

```
POST /repos/{owner}/{repo}/pulls/{pull_number}/reviews
```

Body:
```json
{
  "body": "AIDA Code Review Summary: 3 critical, 12 important, 8 minor issues",
  "event": "REQUEST_CHANGES",
  "comments": [{
    "path": "src/models.rs",
    "line": 142,
    "body": "**CRITICAL:** Function `process_requirement` is 285 lines. Split into: prefix resolution, number generation, formatting."
  }]
}
```

Events: `APPROVE`, `REQUEST_CHANGES`, `COMMENT`

### 6.4 Danger

A CI-integrated code review automation tool:
- Runs during CI and posts automated feedback to PRs
- Can enforce: CHANGELOG updates, PR description requirements, label usage, anti-pattern detection
- Plugin architecture for extending checks
- Ruby-based (also has JS and Swift variants)
- **Relevance:** Lower -- AIDA's skill-based approach is more flexible

### 6.5 Recommended Output Strategy for the Skill

The review skill should produce output in multiple formats:

1. **Console report** (always) -- Structured markdown shown to the user in terminal
2. **Markdown file** (`docs/code-review-report.md`) -- Persistent, diffable, can be committed
3. **SARIF file** (optional, `--sarif`) -- For GitHub Code Scanning integration
4. **GitHub PR comments** (optional, `--pr N`) -- Direct posting via `gh` CLI

---

## 7. Proposed Rule Categories for the Enhanced Skill

Based on this research, here is a comprehensive rule taxonomy. Each rule has an ID prefix indicating its category:

### TRACE -- Traceability Rules
| ID | Name | Severity | What It Checks |
|----|------|----------|----------------|
| TRACE-001 | Missing trace comment | Important | Source file >100 lines with no `// trace:` comment |
| TRACE-002 | Orphan trace | Important | Trace comment references a deleted/rejected requirement |
| TRACE-003 | Incomplete forward trace | Minor | Requirement in "approved" status with no implementing code |
| TRACE-004 | Stale trace | Minor | Traced requirement status doesn't match code state |

### COMPLEXITY -- Complexity Rules
| ID | Name | Severity | What It Checks |
|----|------|----------|----------------|
| COMPLEXITY-001 | God file | Critical | File exceeds 1000 lines of code |
| COMPLEXITY-002 | Long file | Important | File exceeds 500 lines of code |
| COMPLEXITY-003 | God function | Critical | Function exceeds 100 lines |
| COMPLEXITY-004 | Long function | Important | Function exceeds 50 lines |
| COMPLEXITY-005 | Deep nesting | Important | Nesting depth exceeds 4 levels |
| COMPLEXITY-006 | High cyclomatic | Important | Cyclomatic complexity > 20 |
| COMPLEXITY-007 | High cognitive | Important | Cognitive complexity > 25 |
| COMPLEXITY-008 | Too many arguments | Minor | Function has > 7 parameters |

### DEAD -- Dead Code Rules
| ID | Name | Severity | What It Checks |
|----|------|----------|----------------|
| DEAD-001 | Unused public function | Important | `pub fn` with zero callers in project |
| DEAD-002 | TODO/FIXME/HACK markers | Minor | Provisional code markers still present |
| DEAD-003 | Unused dependencies | Important | Declared but unused crate dependencies |
| DEAD-004 | Commented-out code | Minor | Blocks of commented code (>5 lines) |

### ERROR -- Error Handling Rules
| ID | Name | Severity | What It Checks |
|----|------|----------|----------------|
| ERROR-001 | Bare unwrap | Critical | `.unwrap()` in non-test code |
| ERROR-002 | Empty expect | Important | `.expect("")` with no explanation |
| ERROR-003 | Swallowed error | Critical | `let _ = might_fail()` without logging |
| ERROR-004 | Missing context | Important | `Err(e)` without `.context()` wrapping |
| ERROR-005 | Panic in production | Critical | `panic!()`, `todo!()`, `unimplemented!()` outside tests |

### SECURITY -- Security Rules
| ID | Name | Severity | What It Checks |
|----|------|----------|----------------|
| SECURITY-001 | Hardcoded secret | Critical | Password, API key, token literals in code |
| SECURITY-002 | Unsafe without safety comment | Important | `unsafe` block without `// SAFETY:` justification |
| SECURITY-003 | Known vulnerability | Critical | Dependency with advisory (cargo-audit) |
| SECURITY-004 | Path traversal risk | Important | User input used in file path construction |
| SECURITY-005 | SQL injection risk | Critical | String interpolation in SQL queries |

### CONSISTENCY -- Consistency Rules
| ID | Name | Severity | What It Checks |
|----|------|----------|----------------|
| CONSISTENCY-001 | Mixed error handling | Minor | Some modules use anyhow, others use custom errors |
| CONSISTENCY-002 | Naming convention violation | Minor | Non-snake_case functions, non-CamelCase types |
| CONSISTENCY-003 | Import style mismatch | Minor | Mix of `use` at top and inline qualified paths |
| CONSISTENCY-004 | Stringly-typed API | Important | `&str` parameter where an enum would be safer |
| CONSISTENCY-005 | Clone storm | Important | >5 `.clone()` calls in a single function |

### TEST -- Test Quality Rules
| ID | Name | Severity | What It Checks |
|----|------|----------|----------------|
| TEST-001 | Untested module | Important | Source module with no `#[cfg(test)]` or test file |
| TEST-002 | Happy path only | Minor | Tests only cover success cases, no error cases |
| TEST-003 | Test depends on external state | Minor | Tests requiring network, filesystem, or database |
| TEST-004 | Missing assertion | Important | Test function with no `assert!` / `assert_eq!` |

### DOCS -- Documentation Rules
| ID | Name | Severity | What It Checks |
|----|------|----------|----------------|
| DOCS-001 | Undocumented public API | Minor | `pub fn` or `pub struct` without `///` doc comment |
| DOCS-002 | Stale doc comment | Important | Doc comment doesn't match current signature |
| DOCS-003 | Suppressed docs | Minor | `#[allow(missing_docs)]` on public items |

### DEPS -- Dependency Health Rules
| ID | Name | Severity | What It Checks |
|----|------|----------|----------------|
| DEPS-001 | Outdated dependency | Minor | Dependency with newer compatible version |
| DEPS-002 | Major version behind | Important | Dependency more than 1 major version behind |
| DEPS-003 | License incompatibility | Critical | Dependency license conflicts with project |
| DEPS-004 | Unmaintained dependency | Important | Dependency with no commits in >2 years |

---

## 8. Gap Analysis: Current vs. Enhanced Skill

The existing `/aida-code-review` skill (already in the codebase) covers the right categories but could be enhanced:

### What the Current Skill Does Well
- 10 review dimensions covering all key areas
- Practical bash commands for each check
- Anti-pattern catalog (God File, Unwrap Forest, etc.)
- SARIF output template
- Integration with AIDA requirements database

### Enhancement Opportunities

1. **Formal rule IDs** -- Each finding should have a stable ID (e.g., `COMPLEXITY-001`) for tracking and suppression
2. **rust-code-analysis integration** -- Compute actual cyclomatic/cognitive complexity scores instead of approximating from line counts and grep
3. **Pedantic clippy** -- Run `clippy::pedantic` and `clippy::nursery` in addition to defaults
4. **cargo-deny integration** -- License compliance and source trust checking (not just cargo-audit)
5. **Structured JSON intermediate** -- Tool output should be JSON/SARIF first, then rendered to markdown/HTML
6. **Diff-aware mode** -- Only report issues in changed files (for PR reviews), not the entire codebase
7. **Suppression mechanism** -- Allow `// aida-review:ignore RULE-ID` comments to suppress specific findings
8. **Trend tracking** -- Compare current report against previous reports to show improvement/regression
9. **Severity scoring** -- Weighted score (critical=10, important=5, minor=1) for overall health metric
10. **GitHub PR integration** -- Post findings as PR review comments via `gh` CLI

---

## 9. Recommended Implementation Plan

### Phase 1: Enhanced Automated Checks
- Add pedantic/nursery clippy to the automated scan
- Integrate cargo-deny (licenses, bans, advisories, sources)
- Add rust-code-analysis for complexity metrics
- Formalize rule IDs for all findings

### Phase 2: Structured Output
- Produce findings as JSON array with rule ID, severity, file, line, message, suggested fix
- Render JSON to markdown report (console + file)
- Generate SARIF output for GitHub integration
- Add HTML report with dark-theme diff viewer

### Phase 3: Smart Review
- Diff-aware mode: only analyze changed files relative to a base branch
- Cross-reference all findings against AIDA requirements database
- Compute forward and backward traceability metrics
- Track review scores over time

### Phase 4: Integration
- GitHub PR comment posting via `gh api`
- SARIF upload to GitHub Code Scanning
- Auto-create AIDA requirements for critical findings
- CI/CD integration documentation

---

## Sources

### Tools
- [Clippy - GitHub](https://github.com/rust-lang/rust-clippy)
- [cargo-audit - GitHub](https://github.com/RustSec/cargo-audit)
- [cargo-deny - GitHub](https://github.com/EmbarkStudios/cargo-deny)
- [cargo-machete - GitHub](https://github.com/bnjbvr/cargo-machete)
- [cargo-udeps - GitHub](https://github.com/est31/cargo-udeps)
- [cargo-geiger - GitHub](https://github.com/geiger-rs/cargo-geiger)
- [cargo-outdated - GitHub](https://github.com/kbknapp/cargo-outdated)
- [cargo-bloat - GitHub](https://github.com/RazrFalcon/cargo-bloat)
- [tokei - GitHub](https://github.com/XAMPPRocky/tokei)
- [rust-code-analysis - GitHub](https://github.com/mozilla/rust-code-analysis)
- [rust-code-analysis docs.rs](https://docs.rs/rust-code-analysis/latest/rust_code_analysis/)
- [CodeQL - GitHub](https://github.com/github/codeql)

### Standards & Best Practices
- [Google Engineering Practices - Code Review](https://google.github.io/eng-practices/review/)
- [Google - What to Look For in a Code Review](https://google.github.io/eng-practices/review/reviewer/looking-for.html)
- [Google - The Standard of Code Review](https://google.github.io/eng-practices/review/reviewer/standard.html)
- [The Ultimate 2025 Code Review Checklist: 8 Pillars - DocuWriter](https://www.docuwriter.ai/posts/code-review-checklist)
- [Code Review Best Practices 2025 - Group107](https://group107.com/blog/code-review-best-practices/)
- [SonarSource - Cognitive Complexity](https://www.sonarsource.com/blog/cognitive-complexity-because-testability-understandability/)

### Traceability
- [DO-178C Requirements Traceability - Parasoft](https://www.parasoft.com/learning-center/do-178c/requirements-traceability/)
- [Traceability in Compliance Projects - Trace.space](https://www.trace.space/blog/traceability-in-compliance-projects)
- [ISO 26262 Requirements Traceability - Parasoft](https://www.parasoft.com/learning-center/iso-26262/requirements-traceability/)

### Integration Formats
- [SARIF Support for GitHub Code Scanning](https://docs.github.com/en/code-security/code-scanning/integrating-with-code-scanning/sarif-support-for-code-scanning)
- [SARIF 2.1.0 Specification - OASIS](https://docs.oasis-open.org/sarif/sarif/v2.1.0/sarif-v2.1.0.html)
- [Reviewdog - GitHub](https://github.com/reviewdog/reviewdog)
- [Danger - GitHub](https://github.com/danger/danger)
- [GitHub Pull Request Reviews API](https://docs.github.com/en/rest/pulls/reviews)
- [Rustc Lints - Warn by Default](https://doc.rust-lang.org/rustc/lints/listing/warn-by-default.html)
