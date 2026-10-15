# AGY brief: skill-catalog slop audit (consolidation, not building)

**Date filed**: 2026-05-29
**Target reader**: Antigravity (AGY1 or AGY2)
**Time budget**: 45–60 minutes
**Outcome wanted**: a keep / merge / cut recommendation for the AIDA skill catalog, with the 575/577/481 trio explicitly evaluated. NO code changes — this is analysis.

## Why this brief

The operator steered (2026-05-29): the article→SPIKE→ship firehose risks "adding too much slop — overlapping, noisy, or confusingly ambiguous interface." Three skill-adding branches are queued (TASK-575 hardens skill frontmatter, TASK-577 adds `/aida-insights`, STORY-481 adds `/aida-techdebt`) and their merge is **on hold pending this audit**. The question isn't "can we merge them" (we can) — it's "do they earn their place, or do they muddy the interface?"

## What to audit

The full `.claude/skills/` + `.claude/commands/` catalog (and the `aida-core/templates/` masters they mirror). Inventory every skill, then assess overlap.

Known near-neighbors worth scrutinizing:
- `aida-learn` (capture a rule from a mistake)
- `aida-doctor` (diagnose/heal state drift)
- `aida-recover` (recovery flows)
- `aida-drain-queue` (drive the queue)
- `aida-techdebt` (STORY-481, pending — end-of-session duplication scan)
- `aida-insights` (TASK-577, pending — monthly telemetry-pattern view; wraps `aida usage`)
- `aida-digest` (narrative work digest)
- `aida-punt` / `aida-advise` (orchestrator-internal)
- `aida-capture` / `aida-doc`

## The questions to answer per skill (and per overlap cluster)

1. **Distinct trigger?** Does it activate on a request shape no other skill covers? Or does it overlap a neighbor's trigger (e.g. does `/aida-insights` overlap `/aida-digest`? does `/aida-techdebt` overlap `/aida-doctor` or a plain code-review?).
2. **Earns a top-level `/` slot?** Or could it be a flag/mode on an existing skill (e.g. `/aida-doctor --techdebt` instead of a separate `/aida-techdebt`)?
3. **Wraps something thin?** `/aida-insights` reportedly wraps `aida usage` — is the skill adding real workflow value, or is it a thin shell over a CLI command the user could just run?
4. **Catalog cost.** Each skill's name+description loads into Claude's skill-selection context every session (per Claude Code's skill-listing budget). More skills = more selection noise. Is the marginal skill worth its slice of that budget?

## The trio verdict specifically

For TASK-575 / TASK-577 / STORY-481, recommend one of:
- **Ship as-is** — each is distinct and earns its slot.
- **Merge/fold** — e.g. fold `/aida-insights` into `/aida-digest`, or `/aida-techdebt` into `/aida-doctor`, and ship only the consolidated form.
- **Cut** — the capability doesn't earn a skill; close the spec as rejected (the observation that prompted it stays in the graph).
- **Defer** — park until there's evidence of real use-demand.

TASK-575 (frontmatter hardening on destructive skills) is probably the most defensible — it's safety, not new surface. Evaluate it separately from the two net-new skills.

## What this audit is NOT

- Not a rewrite. Don't change any skill files.
- Not a merge. The trio stays unmerged until the operator rules on your recommendation.
- Not a defense of the status quo. "We already have N skills" is not a reason to add the N+1th; it might be a reason to cut back to N-3.

## Deliverable

`docs/aida/skill-catalog-audit-2026-05-29.md` — a table (skill | trigger | overlaps | keep/merge/cut/defer | rationale) + a one-paragraph verdict on the trio. Commit to a branch + report.

## Desired return shape

When you reply (via Joe):
1. **Trio verdict** (one line each for 575/577/481: ship / merge-into-X / cut / defer)
2. **The 2-3 sharpest overlaps** you found in the existing catalog (candidates for consolidation independent of the trio)
3. **Net catalog delta** you'd recommend (e.g. "+1 net: ship 575, fold 577→digest, cut 481")

Under 400 words prose. The audit table is the deliverable.

---

trace:from-strategic-recompose-round-3 | ai:claude-master-advisor-asking-agy
