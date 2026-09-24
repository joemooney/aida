---
description: Run /aida-derisk.
---
# De-risk a Supervised Spec

Treat `execution_mode = guided` as a *not-yet-de-risked* state, not a permanent
label, and — where it can be done honestly — make the spec drain-eligible.
Takes one spec id: `/aida-derisk <SPEC>`.

## Instructions

Follow the workflow in `.claude/skills/aida-derisk/SKILL.md`:

1. Read the spec + its graph; note the current `execution_mode` and *why* it was
   fenced (`aida show <SPEC> --full`, `aida graph tree <SPEC>`).
2. Classify the risk: **decision risk** (undecided forks — fully de-riskable) vs
   **blast-radius risk** (a wrong headless impl is costly — partly de-riskable),
   and whether the spec is a **hard-limit class** (destructive/irreversible,
   drain self-modification, or removes a security safeguard) that can never be
   flipped to `drain`.
3. Surface each undecided fork as a structured `AskUserQuestion` (options +
   consequences + recommendation + an "enter your own" escape).
4. Record each load-bearing decision as a traceable ADR
   (`aida add --type decision` + `aida rel add <ADR> <SPEC> --type references`).
5. Fold the decisions AND the testable safety gates (round-trip/corruption test,
   dry-run, backup-before-write, project-local-first, never-touch-sensitive-paths)
   into the acceptance criteria; `aida lint <SPEC>` to keep them concrete.
6. Propose the new `execution_mode` with rationale — `drain` only when fully
   decided AND safety is testable/CI-gated AND not a hard-limit class; else
   `drive`/`guided`. On the operator's explicit OK, write it
   (`aida edit <SPEC> --mode <mode>`). **Never write the mode silently, and
   never flip a hard-limit-class spec to `drain`.**
7. Report the ADR ids, the gates now guarding the spec, and the mode transition
   (from → to, or held + the class that held it).

This skill only moves a spec across the existing guided/drive/drain boundary by
making it ready — it does not implement the spec and does not touch the drain
engine or pickup fence. Pairs with `/aida-guided-implement` (which implements a
supervised spec) and `aida groom` (which sets the initial mode).
