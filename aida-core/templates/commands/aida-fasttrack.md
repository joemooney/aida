---
description: "The fast lane for small work — TRIVIAL (cosmetic/doc/one-line, CI-only, review skipped) or EXPRESS (easy bug / small feature, full CI + reviewer, fast because reliably routed). An eligibility litmus picks the tier; a punt-out invariant keeps it honest."
---
# Fasttrack a small change — two tiers on one lane

The fast lane is two named tiers on one pipe, told apart by one question: does it
skip the human-review ceremony? TRIVIAL (cosmetic/doc/one-line) skips it — CI is
the only gate. EXPRESS (easy bug / small feature with real behavior) keeps full
CI + a reviewer; it is fast because it is *reliably routed*, not less gated.

## Instructions

Follow the workflow in `.claude/skills/aida-fasttrack.md`:

1. Read `$ARGUMENTS` as the change description.
2. Run the **eligibility litmus** (bounded / low-blast-radius / reversible /
   excluded-classes-absent). Fail any ⇒ file normally, not in the lane. Pass ⇒
   pick the tier: also-cosmetic/doc/one-line ⇒ trivial; real-behavior-but-bounded
   ⇒ express. When in doubt, choose the more-gated tier (express over trivial).
3. File + queue it tagged for the chosen tier (capture the SPEC-ID):
   - Trivial: `aida fasttrack "<desc>" --type <task|bug>` (Approved + queued +
     `batch:fasttrack` + `lifecycle:no-review`).
   - Express: `aida add "<desc>" --status approved --queue --batch express --type <task|bug|story>`
     (NO `lifecycle:*` — full gate).
4. Implement on a fresh branch off `origin/main`. Trivial = reduced gate
   (build + `fmt --check` + `clippy -D correctness` + smoke); express = full gate
   + a CI reviewer.
5. Commit with the `(SPEC-ID)` trailer, open a PR, merge **only on green CI**
   (express also needs the reviewer verdict); `aida pull` to auto-bump.

The one hard rule: neither tier skips CI. **Punt-out invariant** — if the work
turns out non-trivial, punt to NeedsAttention + file a finding + drop the lane
tag so it re-enters normal review. Never silently re-gate.

ARGUMENTS: $ARGUMENTS
