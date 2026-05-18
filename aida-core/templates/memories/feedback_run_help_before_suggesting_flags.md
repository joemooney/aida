---
name: run-help-before-suggesting-flags
description: Run `<cmd> --help` before suggesting CLI flags — verify the surface, don't pattern-match from mental models.
propagation: scaffolding-pack
metadata:
  type: feedback
---

When suggesting CLI flags, do not pattern-match from mental models or analogous tools. Run `<command> --help` first, or ask the user to paste it. Otherwise you create UX friction by suggesting flags that do not exist — and the failed attempt produces an `unexpected argument` error that wastes a round-trip.

**Why:** A guessed flag is often on the wrong surface entirely — the command's real shape differs from what an analogous tool would suggest. Verifying costs ten seconds; guessing costs the user an error and a re-ask.

**How to apply:** When the user asks how to do something:

1. If the command is unfamiliar, run `<command> --help` before responding.
2. If you are confident from prior context, still confirm the specific flag exists in the help output.
3. If you cannot run a shell, ask the user to paste `--help`.
4. Cite the actual flag names from help, not from analogous tools.

The discipline generalizes: read the actual skill template before specifying a skill's UX; run the actual diagnostic command before asserting state; inspect the actual config before describing it. Verify the artifact — don't reason from analogy.

Composes with [[failed-flag-attempts-are-signals]] (when flags genuinely fail, file them — don't artificially generate failed attempts by guessing) and [[verify-before-filing]].
