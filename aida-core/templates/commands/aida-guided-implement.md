---
description: Run /aida-guided-implement.
---
# Guided Keystone Implementation

Drive a structured, step-by-step decision dialog for a keystone / architecture
/ security / supervised spec that must NOT run unattended. Takes one spec id:
`/aida-guided-implement <SPEC>`.

## Instructions

Follow the workflow in `.claude/skills/aida-guided-implement/SKILL.md`. The HYBRID flow:

**PHASE 1 — major decisions up front**
1. Read the spec + its graph (`aida show <SPEC>`, `aida graph tree <SPEC>`, `aida graph blocked-by <SPEC>`, `aida graph impact <SPEC>`, existing ADRs/comments).
2. Identify the MAJOR architectural / security / keystone forks (the load-bearing choices) — typically one to four; do not elevate mechanical choices.
3. Ask EACH fork via a structured `AskUserQuestion`: the decision + why it matters, 2–4 concrete options with their consequences, a recommendation + rationale, and an "enter your own" prose escape (mirror the finish-checkpoint rubric in `.aida/discipline/session-discipline.md`).
4. Record each answer as a traceable ADR: `aida add --type decision` (linked to the spec via `aida rel add <ADR> <SPEC> --type references`), or `aida comment add <SPEC>` for a secondary call.
5. Summarise + agree the overall approach, then `aida edit <SPEC> --status in-progress` — BEFORE any code.

**PHASE 2 — build**
6. Implement the agreed design with `// trace:<SPEC> | ai:claude` comments. Pause just-in-time (another `AskUserQuestion`) ONLY for a NEW smaller fork that emerges; never re-ask a settled decision.

**PHASE 3 — finish**
7. Commit with the `(SPEC-ID)` trailer (ADR ids in the body), rebase onto `origin/main`, `aida queue done <SPEC>`, open the PR with `/aida-pr`.
8. Finish with the off-ramp checkpoint, not an auto-merge: implementation complete and submitted for review — PR #<N>; CI is running / review pending; this session's work is finished and the operator is free to close it; remaining steps happen through the PR (review -> merge); for a KEYSTONE the merge stays human/advisor; resume only if review requests changes with `aida queue work <SPEC> --resume`; list the ADRs recorded. Do not claim "done" before CI is green.

This is an INTERACTIVE skill — if there is no human at the keyboard, stop.
Launch via `aida queue work <SPEC> --guided` or invoke `/aida-guided-implement <SPEC>` directly.
