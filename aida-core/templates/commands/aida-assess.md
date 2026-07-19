---
description: "Headless advisor INTAKE pass: a cold-boot advisor agent reads all open specs, applies worth-doing judgment, and proposes approve/reject/park/queue per spec. The advisor-side analog of `/aida-burndown` (which fires implementers)."
---
# AIDA Assess

Headless advisor INTAKE pass: a cold-boot advisor agent reads all open specs,
applies worth-doing judgment, and proposes approve/reject/park/queue per spec.
The advisor-side analog of `/aida-burndown` (which fires implementers).

## Usage

```
aida groom                  Propose dispositions for every open spec — writes NOTHING.
aida groom --apply          Execute the proposed approvals + groom the queue.
aida groom --dry-run        Show the candidate fence + the claude -p command, launch nothing.
```

Guardrails (compose with the `[intake]` config; flags override config for a run):

```
--max-approvals N           Cap how many drafts the advisor may approve.
--only-tag TAG              Only weigh specs carrying TAG.
--exclude-tag TAG          Never weigh specs carrying TAG.
--risk low|medium|high     Exclude candidates riskier than this ceiling (default: medium).
--then-drain               After queuing, chain a burndown drain (overrides on_apply).
--permission-mode MODE     Claude permission mode (default: bypassPermissions).
```

This command launches `claude -p "/aida-assess"` headless. It is not the skill
itself — the skill (`.claude/skills/aida-assess.md`) is the judgment the spawned
advisor follows.

## Instructions (for the spawned advisor)

Follow the workflow in `.claude/skills/aida-assess.md`:

1. Read your fence — the specs in `$AIDA_INTAKE_CANDIDATES` (the launcher
   already excluded the do-not-approve classes + `needs-human`/`strategic`).
2. Apply the disposition bias (`$AIDA_INTAKE_DISPOSITION_BIAS`): approve-eligible
   (default) / park-aligned (read the strategic frame first) / park-conservative.
3. Propose a disposition per spec — approve / reject / park-for-human / queue —
   each with one line of reasoning. **Park when unsure; you are a cold boot.**
4. Output the proposal table. If `AIDA_INTAKE_APPLY` is not `1`, STOP — propose
   mode writes nothing.
5. Under `--apply`: `aida edit --status approved` within the fence (≤ max-approvals,
   never a parked spec), reject the stale ones, then groom the queue with
   `aida backlog groom --pickable --apply --risk "$AIDA_INTAKE_RISK"`. If
   `on_apply=drain`, chain `aida burndown run`.
6. Report what changed (approved / rejected / parked / queued) + check
   `aida findings list` if a drain shelved anything.

The load-bearing rule: **propose-mode is the gate.** Autonomy-eligible is not
worth-doing — a cold-boot advisor blessing everything completable undoes the
operator's curation.

ARGUMENTS: $ARGUMENTS
