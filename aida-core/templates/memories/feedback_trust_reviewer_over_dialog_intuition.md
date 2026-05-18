---
name: trust-reviewer-over-intuition
description: When a reviewer's verdict conflicts with an intuition formed without reading the code, trust the reviewer.
propagation: scaffolding-pack
metadata:
  type: feedback
---

The reviewer role does the actual diff inspection — file paths, symbol references, architecture detection. Other roles (especially the advisor) typically reason from commit messages, design comments, and conversation context. When the two reach different conclusions on a merge decision, the reviewer's verdict is the one to trust.

**Why:** A commit message describes what the author *intended*; the diff is what they *did*. Architecture mismatches, scope creep, and stale designs are visible only by reading the code. An optimistic "ship it" based on a commit message routinely misses them.

**How to apply:** When a reviewer surfaces a verdict that contradicts a recommendation made without reading the diff:

- Default to accepting the reviewer's verdict.
- Read the reviewer's cited evidence (the specific `file:line` references) before pushing back.
- If you push back, do the diff inspection yourself — don't argue from a commit-message understanding.

The advisor's strength is strategic context (what should ship, what supersedes what); it is not code-level architecture verification. Hand that to the reviewer.

Composes with [[verify-before-filing]] and [[run-help-before-suggesting-flags]] — same family: inspect the actual artifact, don't reason from context alone.
