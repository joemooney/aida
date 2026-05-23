# Substrate as bouncer, not passive rules

**Last updated**: 2026-05-22  
**Principle Trace**: `feedback_substrate_as_bouncer_not_rules` | `TASK-481` | `TASK-480`

In high-velocity development environments, relying on passive developer guidelines (such as rule lists in a README or a text-only memory file) is fragile. Humans in a hurry and developer-agents alike routinely bypass, ignore, or overlook these passive rules, leading to common friction traps. 

The AIDA philosophy solves this by shifting from **passive rules** to **active bouncers** built directly into the workspace *substrate*.

---

## The Core Principle

A **bouncer** is a hard, mechanical gate embedded directly in the development lifecycle (such as Git hooks, compilers, pre-commit scripts, or IDE rules). 

Instead of asking you (or an agent) to remember a rule:
1. **The substrate physicalizes the boundary**: It actively intercepts the action (e.g. committing).
2. **It fails fast and hard**: It refuses to complete the action if the boundary is violated.
3. **It educates immediately**: It prints a highly informative error explaining *why* it refused the action and points to the exact discipline or solution.

By turning rules into bouncers, we automate discipline. Bouncers protect the codebase from structural rot without relying on human willpower or prompt adherence.

---

## The Pre-Commit Gitignored Bouncer Hook

The first line of defense is the pre-commit bouncer (`.git/hooks/pre-commit`), which protects projects from intermediate build product contamination.

### The Problem it Solves
A common friction trap occurs when developer-agents (such as Claude Code) attempt to solve errors by directly editing compiled build artifacts, generated types, or cached databases (which are gitignored) instead of modifying the true source files. This produces non-reproducible local fixes that break upon the next clean build.

### The Hook's Behavior
Upon running `git commit`, the pre-commit bouncer:
1. Scans all staged files using `git status --porcelain`.
2. Runs `git check-ignore --no-index` on each to detect gitignored paths.
3. If any staged file is gitignored, the commit is **actively refused** with a structured error:
   ```
   Refusing commit: target/debug/build/... is gitignored. Editing intermediate
   build products produces non-reproducible fixes. Modify the
   source (or pass --allow-intermediate to override). See:
   docs/aida-discipline/substrate-as-bouncer.md
   ```

### The Deliberate Escape Hatch
Bouncers must always remain helpful, not obstructive. If a developer has a legitimate, emergency reason to commit a gitignored file, they can deliberately bypass the bouncer using either:
- The `--allow-intermediate` flag: `git commit --allow-intermediate`
- The environment variable: `AIDA_ALLOW_INTERMEDIATE=1 git commit`

### The .aida-store Exemption
The bouncer automatically exempts the orphan `.aida-store` branch and its worktree, since it deliberately stores project requirement state in gitignored paths.

---

## Sibling: The Reviewer-Phase Gate (TASK-480)

Local pre-commit hooks are powerful but can be bypassed (e.g., using `git commit --no-verify`). To guarantee enforcement at scale, AIDA implements a multi-layered bouncer strategy.

The sibling gate (**TASK-480**) operates at the **Reviewer phase**:
- During `aida pr ship` or CI/CD execution, a reviewer-phase bouncer scans the commit range of the incoming branch.
- If any commit in the range touched a gitignored file without explicit bouncer bypass metadata, the reviewer **refuses to approve the PR**.
- This ensures that local shortcuts never reach the main branch.

---

## Summary of Bouncer Habits

When writing code or designing workflows under AIDA:
- **Do not write a rule when you can write a bouncer**.
- **Make bouncers loud and informative**: They must explain the rationale and point to the discipline document.
- **Provide explicit escape hatches**: Keep control with the human developer while keeping the defaults safe.
