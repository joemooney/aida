---
name: verify-before-filing
description: Before filing a spec from user friction, verify the symptom's actual cause — it may be timing/visibility, not a missing capability.
propagation: scaffolding-pack
metadata:
  type: feedback
---

When a user reports friction ("I had to do X manually", "this didn't fire automatically", "why doesn't AIDA have Y?"), the first instinct is to file a TASK proposing a new capability. Pause before filing — diagnose first. The friction may be timing, visibility, or state confusion, not a missing capability.

**Why:** Speculative filings premised on a "broken" flow that was not actually broken cost a wrong spec that has to be rejected, plus the design time spent on it. A ten-second state query is far cheaper.

**How to apply:** When the user says something like "I had to do X manually" or "this didn't happen automatically", run the actual diagnostic before filing:

- `gh pr view <N> --json state,mergedAt` — is the PR actually unmerged?
- `aida show <SPEC>` — is the spec actually in the status assumed?
- `aida session leases --all` — is the session actually still active?
- `git log -1 --oneline origin/main` — has the change already landed?

A subagent's self-diagnosis — especially a claim about its own environment or execution context — is a *hypothesis to verify with a direct query*, never a fact to file on.

**Pattern to avoid:** friction → instant filing → speculative design → user pushes back → diagnostic finally reveals the friction was a no-op.

Composes with [[failed-flag-attempts-are-signals]] (distinguish "this didn't work" — a real signal — from "I didn't see it happen" — a visibility issue) and [[trust-reviewer-over-intuition]].
