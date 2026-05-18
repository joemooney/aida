---
name: refinements-must-be-acceptance-criteria
description: A design refinement on a filed spec must edit the acceptance criteria (or file a follow-up) — comments are not binding on implementers.
propagation: scaffolding-pack
metadata:
  type: feedback
---

After a spec is filed, refinements arise — clearer wording, tweaked design, corrected detail. These are often captured as **comments** on the spec. Comments are *not* binding on implementers. The implementer's contract is the spec's `## Acceptance` checkbox list; comments are background context.

**Why:** An implementer correctly follows the spec they were given. A refinement buried in a comment never reaches them as a requirement, so it silently does not ship — and the gap surfaces only when the result is seen in the wild.

**How to apply:** When iterating with the user on an already-filed spec:

1. **Substantive refinements** (anything that changes the implementer's output): edit the original acceptance list, or file a follow-up spec that explicitly supersedes the original behavior.
2. **Background context** (rationale, examples, forks considered): a comment is fine.
3. **Discriminator:** *would the implementer's output differ based on this refinement?* If yes, it belongs in acceptance. If no, a comment is fine.

Even better — when a refinement-touched spec ships, run the resulting code or output and confirm the refinement landed. If it did not, that is a bug worth filing immediately. Don't assume the comment did its job.

Composes with [[pause-for-design-input]] (the input you gather must be captured in a binding form) and [[trust-reviewer-over-intuition]] (verify the artifact, don't assume the description matches reality).
