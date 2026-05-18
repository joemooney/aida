---
name: failed-flag-attempts-are-signals
description: When an agent tries a non-existent flag, that error IS the diagnostic — a discoverability finding, not optional polish.
propagation: scaffolding-pack
metadata:
  type: feedback
---

When an agent attempts a flag that does not exist and gets `error: unexpected argument`, that error is diagnostic signal, not noise. It means the agent's mental model of the command's surface diverged from reality, and the command's actual output did not redirect it. That is a UX / discoverability bug.

**Why:** An agent's failed attempt is raw expectation-vs-reality data — it has not been filtered through a human's "is this worth mentioning?" Its signal value is *higher* than a user-reported UX complaint, not lower.

**How to apply:**

- Default to filing. Treat a flag-attempt error as a discoverability finding. Ask "should we file this?", not "does this bother you?" — the latter wrongly frames it as a taste call.
- Even when the right command exists, the gap is that the *wrong* command's output did not lead there. Both can be true: the right way exists AND the discovery path is broken.
- Lead the fix proposal with the evidence: "an agent attempted `--X` and got error Y; that suggests Z about the mental model."
- Generalize beyond flags — failed subcommands, agents asking where to find inline-surfaced info, agents writing duplicate files because they could not find existing ones are all discoverability signals.

Composes with [[verify-before-filing]] (distinguish a real failure from a missed notification) and [[run-help-before-suggesting-flags]] (don't *artificially* generate failed attempts by guessing).
