---
name: aida-derisk
description: De-risk a supervised spec so it can climb DOWN the autonomy ladder (guided/operator → drive/drain).
allowed-tools:
  - Bash
  - Read
  - Grep
  - AskUserQuestion
---

# AIDA De-risk Skill

## Purpose

`execution_mode = guided` (or `operator` / `decide`) should mean **"not yet
de-risked"**, not "keyboard-only forever". A spec whose design forks are
decided and whose safety properties are *testable* is drainable. This skill is
the step that converts a supervised spec into a drain-eligible one — or proves
it must stay on the keyboard, and says why.

It is the advisor-driven counterpart to `/aida-guided-implement`: that skill
*implements* a keystone under supervision; this one *removes the supervision
requirement* where it can be honestly removed, by locking the decisions and the
safety gates into the substrate.

**Nothing here touches the drain engine or the pickup fence.** It moves a spec
across the existing `guided`/`drive`/`drain` boundary by making it *ready*,
using only `aida edit --mode`, `aida add --type decision`, `aida rel add`, and
`aida edit`/`aida comment` on the acceptance. Takes one spec id:
`/aida-derisk <SPEC>`.

## Why a spec is supervised — the two risk classes

Every `guided`/`operator`/`decide` fence traces to one (or both) of:

1. **Decision risk** — undecided design forks that need a human call. This is
   fully de-riskable: decide the forks, and the remaining implementation is
   mechanical.
2. **Blast-radius risk** — even a well-specified implementation, done wrong
   headless, is costly or hard to reverse. This is *partly* de-riskable: raise
   the floor with testable safety gates the drain's CI + reviewer enforce.

## The hard limit — classes that stay on the keyboard

Some specs must NOT be flipped to `drain` even when fully-specified. If the spec
is in one of these classes, propose `drive` at most (never `drain`), and name
the class in the rationale:

- **Destructive / irreversible actions** — deletes data, removes a live
  worktree, rewrites history, force-pushes, publishes/releases. A wrong headless
  run loses work. (Example archetype: reaping a *live* session's worktree.)
- **Drain self-modification** — the spec changes the drain-invocation layer
  itself (queue-work / auto-complete / autoprogress / drain start-stop / the
  pickup fence). The drain building the drain is a foot-gun; keep eyes on it.
- **Security-posture that removes a safeguard** — anything that could *widen*
  sandbox/approval access (e.g. a bypass path). Adding a *safer* tier is fine;
  removing a guard is keyboard.

When in doubt, do not flip. A false "drainable" is worse than a conservative
"still guided".

## Workflow

1. **Read the spec + its graph.** `aida show <SPEC> --full`,
   `aida graph tree <SPEC>`, `aida graph blocked-by <SPEC>`, plus existing ADRs
   / comments. Note the current `execution_mode` and *why* it was fenced.
2. **Classify the risk.** Decide which of the two classes (decision /
   blast-radius) apply, and whether the spec is in a **hard-limit class** above.
   If it is a hard-limit class, the best achievable target is `drive` — record
   that up front so the dialog is honest about the ceiling.
3. **Surface + decide the forks.** Identify the undecided design forks (usually
   1–4; do not elevate mechanical choices). Ask EACH via a structured
   `AskUserQuestion`: the decision + why it matters, 2–4 concrete options with
   consequences, a recommendation + rationale, and an "enter your own" prose
   escape (mirror the finish-checkpoint rubric in
   `.aida/discipline/session-discipline.md`).
4. **Record each decision durably.** One traceable ADR per load-bearing fork:
   `aida add --type decision --title "..." --status approved` then
   `aida rel add <ADR> <SPEC> --type references`. Secondary calls may be a
   `aida comment add <SPEC> "..."`. A decision that lives only in chat does not
   de-risk anything — it must be in the substrate.
5. **Fold the decisions AND the safety gates into the acceptance.** Rewrite the
   spec's acceptance criteria (`aida edit <SPEC> --description ...` or an
   acceptance comment) so that:
   - each decided fork is now a fixed criterion (no longer open), and
   - the blast-radius risks are covered by **testable gates the drain enforces**:
     e.g. a round-trip/corruption test, a dry-run-and-diff, backup-before-write,
     atomic swap, project-local (`--local`) first, never-touch-sensitive-paths.
   A gate that is not testable/CI-checkable does not lower blast-radius risk —
   keep the spec supervised until it is.
6. **Lint the result.** `aida lint <SPEC>` — the acceptance should read as
   concrete, testable behavior, not vague intent. Un-vague anything it flags.
7. **Propose the new execution_mode — never silent.** State the verdict to the
   operator with its rationale:
   - `drain` — the spec is fully-decided AND its safety is testable/CI-gated AND
     it is NOT a hard-limit class.
   - `drive` — fully-decided but a hard-limit class (or a residual blast-radius
     the CI gate cannot fully cover): implement supervised, merge stays human.
   - `guided` (unchanged) — forks remain open, or the safety gates are not yet
     testable.
   Then, on the operator's confirmation, write it:
   `aida edit <SPEC> --mode <drain|drive|guided>` (loosening to a broader mode
   may need `--force`; tightening is free). **Do not write the mode without an
   explicit operator OK** — the whole point is that de-risking is a decision,
   not a default.
8. **Report** what was decided (with the ADR ids), what gates now guard the
   spec, and the mode transition (from → to, or "held, because <class>").

## Guardrails

- This skill NEVER implements the spec — it only makes it ready. Implementation
  is a separate drain / drive / guided pass afterward.
- It NEVER flips a hard-limit-class spec to `drain`.
- It NEVER writes a mode change silently — the operator confirms.
- Prefer conservative: a spec left `guided` costs a supervised session; a spec
  wrongly flipped to `drain` costs a headless mistake on load-bearing work.

## Related

- `/aida-guided-implement` — implements a supervised spec (the counterpart).
- `aida groom` — the advisor disposition pass that sets the initial mode.
- `aida lint` — the EARS-style acceptance-quality lens used in step 6.
- `.aida/discipline/session-discipline.md` — the finish-checkpoint rubric the
  fork dialog mirrors.
- The autonomy ladder: `aida autonomy` (the one-screen map of the tiers).
