<!-- trace:TASK-873 -->
# Demo: lifecycle authority gates

> AIDA gates the **authority to change intent state** — not just what merges.

A runnable demo: [`scripts/demo-lifecycle-authority.sh`](../../scripts/demo-lifecycle-authority.sh).

## Why this demo exists

Most spec/issue tools — Spec Kit, Kiro, Gas Town's Refinery CI queue, Beads —
gate **what merges**: a CI/review queue sits in front of the default branch and
decides whether code lands. That gate is real and valuable, and AIDA has it too.

But the family's gate stops at the merge boundary. The state of a spec — is it
*approved* work? is it *done*? — is, in those tools, either un-modeled or set by
convention. Beads is the clearest case: it has three lifecycle states, but
nothing enforces **who** may set them or that completion follows a merge. They
are labels an agent or human applies by hand.

AIDA's rarer, uncontested enforcement point is the **authority to change intent
state**. Two properties:

1. **Advisor-only promotion.** `Draft -> Approved` moves a spec *into the
   execution pipeline*. That is the advisor's triage decision, and it is gated
   in code (`status_advance_requires_advisor_authority` in `aida-cli/src/main.rs`,
   over the lifecycle predicate in `aida-core/src/lifecycle.rs`). A non-advisor
   identity attempting the promotion is **refused** — not nudged by a convention
   a confident LLM can ignore, but blocked by the binary.

2. **Merge-driven completion.** A spec reaches `completed` only when a commit
   referencing its SPEC-ID lands on the **default branch**. `aida pull` runs the
   auto-bump scan over the commits it brings in, finds the `(SPEC-ID)` trailer,
   and flips `Done -> Completed`. Completion is therefore a **property of git
   ancestry**, not a flag someone types. The lifecycle declares the trigger
   explicitly: `Done --> Completed: merge auto-bump (aida pull)`.

The competitive read (§15, Tier-1) isolated this as genuinely distinctive but
**asserted, not shown**. This demo shows it.

## What the demo does

The script runs entirely against a **throwaway sandbox store**
(`aida sandbox create --path ...`) and never touches your project's real store.
Six steps, each with the command and its output visible:

| Step | Action | Result |
|------|--------|--------|
| 1 | Pre-flight + create the throwaway sandbox | `AIDA_STORE` points at a temp store |
| 2 | A **non-advisor** files a `Draft` spec | Allowed — capture is cheap |
| 3 | The **non-advisor** tries `Draft -> Approved` | **REFUSED** (exit 1, "needs advisor authority") |
| 4 | The **advisor** runs the same promotion | **Succeeds** — `Approved` |
| 5 | An implementer marks it `Done`; the merge mechanism is shown | `Done` (not `Completed`) |
| 6 | Recap + opt-in sandbox cleanup | — |

Step 3 runs with `AIDA_SESSION_ROLE` unset and stdin redirected (non-TTY), so
neither the advisor-role path nor the interactive-TTY carve-out applies — the
refusal is the bare authority gate firing. Step 4 runs the **exact same edit**
as the advisor and it succeeds: *who* ran the command, not *what* the command is,
is the difference.

Step 5 stops at `Done` deliberately. The `Done -> Completed` bump needs a real
merged commit carrying the `(SPEC-ID)` trailer on the default branch — a full
git round-trip (branch -> commit -> push -> merge -> `aida pull`) heavier than a
single-store demo should fake. So the script shows the **mechanism** instead: the
lifecycle's declared trigger and the `aida db reconcile-status` scan that reads
git ancestry. The full round-trip is exercised end-to-end by
[`scripts/aida-demo.sh`](../../scripts/aida-demo.sh).

## The contrast, in one table

| Tool | What it gates | Intent-state authority |
|------|---------------|------------------------|
| Beads | merge (CI), plus 3 lifecycle states | states set **by convention** — unenforced |
| Spec Kit / Kiro / Gas Town Refinery | **what merges** (CI + review in front of main) | not a gated concept |
| **AIDA** | what merges **and** the authority to advance intent state | advisor-only approval + merge-driven completion, enforced in code |

## How to run it

```bash
# Use the in-repo dev build if you are developing AIDA:
aida dev activate

# Walkthrough (Enter-to-continue between steps):
bash scripts/demo-lifecycle-authority.sh

# Non-interactive / CI (destroys the sandbox at the end):
bash scripts/demo-lifecycle-authority.sh --auto-cleanup
```

Prerequisite: `aida` on PATH. No GitHub repo, no network — everything happens in
a local throwaway sandbox store. Cleanup is opt-in: the default keeps the sandbox
so you can poke around (`aida sandbox path --path <dir>` to inspect, then
`aida sandbox destroy --path <dir>` when done).

## Related

- `docs/lifecycle.md` — the full Draft -> Approved -> ... -> Completed state machine.
- `docs/competitive-analysis/2026-05-31-round2-moat-gaps-moves.md` — the moat / gap picture.
- `scripts/aida-demo.sh` — the first-user walkthrough with the full git round-trip.
