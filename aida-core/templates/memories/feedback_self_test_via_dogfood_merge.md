---
name: self-test-via-dogfood-merge
description: A fix that lands via the very system it fixes is the strongest possible validation — look for the dogfood self-test.
propagation: scaffolding-pack
metadata:
  type: feedback
---

When fixing a project's own automation (a merge hook, a status auto-bump, a CI workflow, the session lifecycle), the merge of the fix itself often exercises the new code path. This is the strongest possible validation — the fix tests itself in its own end-to-end shipping cycle, catching integration issues that unit tests miss.

**Why:** Infrastructure fixes have an opportunity feature work does not: the fix can ship through the very plumbing it repairs. Shipping it through a side channel instead means nothing exercises the full path until a real change does — possibly months later.

**How to apply:** When implementing a fix to the project's own automation, ask: *"will the merge of THIS fix exercise the new code path?"*

- Auto-bump fix → the merge triggers auto-bump on the fix's own spec.
- CI workflow fix → the PR's own CI run exercises the workflow.
- Session-lifecycle fix → ending the session exercises the new lifecycle.

If yes, ship the fix through the system being fixed, and note the dogfood moment in the commit message or PR description — as validation evidence and as a pattern others can recognize. If no, write a test that exercises the new path; don't rely on unrelated workflows to catch regressions.

Composes with [[competitive-analysis-is-living-doc]] (durable capture of evidence) and [[verify-before-filing]].
