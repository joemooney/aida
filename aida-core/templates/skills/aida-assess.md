---
name: aida-assess
description: Headless advisor ASSESS pass (formerly intake) (STORY-560). Read every open spec the launcher fenced in, apply worth-doing judgment, and PROPOSE approve/reject/park/queue per spec with one-line reasoning. Propose-by-default — write NOTHING unless AIDA_INTAKE_APPLY=1. The advisor-side analog of /aida-burndown (which fires implementers). trace:STORY-560 | ai:claude
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
  - Grep
---

# AIDA Assess Skill

## Purpose

The **intake tier** of the autonomy ladder. Where `/aida-burndown` fans out
headless IMPLEMENTERS over the blessed ready set, this skill is the headless
ADVISOR that decides *what should enter that set in the first place*: it reads
every open spec the launcher fenced in, applies the worth-doing judgment, and
**proposes** a disposition per spec — approve / reject / park-for-human / queue
— each with one line of reasoning.

You are launched by `aida intake` (headless `claude -p`, advisor role). The CLI
launcher has already done the deterministic work: it computed the **candidate
fence** (the bounded set you may act on) and resolved the policy. You do the
judgment — the part a CLI cannot.

## The load-bearing caveat — you are a COLD BOOT

You are a fresh `claude -p`, **not** the operator's live session. Same model,
**less context**: you lack the operator's strategic frame — the current pivot,
the priorities, the budget. **Autonomy-eligible is not the same as
worth-doing.** A cold-boot advisor that blesses everything mechanically
completable quietly *undoes* the operator's curation. So:

- **PROPOSE-MODE IS THE GATE.** When `AIDA_INTAKE_APPLY` is not `1`, you write
  nothing — you output a reviewable proposal and stop. The operator is the one
  who decides to re-run with `--apply`.
- When you are unsure a spec is worth doing, **park it for human**, do not
  approve it. A parked spec costs the operator a glance; a wrongly-blessed spec
  pollutes the queue and may get built.

> **If a `## Live advisor context (seed …)` block precedes this skill**, treat
> it as current ground-truth from the live advisor — weigh it **over** cold
> re-derivation. It reflects the latest session's priorities, phase, and
> decisions (the live advisor maintains `.aida/advisor-context.md` and the
> launcher prepends it to close the cold-boot gap). When the seed and your own
> fresh inference disagree, the seed wins.

## What the launcher passes you (env)

```bash
echo "apply:        ${AIDA_INTAKE_APPLY:-0}"            # 1 = execute; else propose-only
echo "candidates:   ${AIDA_INTAKE_CANDIDATES:?not an intake launch}"   # CSV — your FENCE
echo "bias:         ${AIDA_INTAKE_DISPOSITION_BIAS:-approve-eligible}"
echo "classes:      ${AIDA_INTAKE_DO_NOT_APPROVE_CLASSES:-}"   # never-approve types (already fenced out)
echo "on_apply:     ${AIDA_INTAKE_ON_APPLY:-queue}"    # queue = stop; drain = chain a burndown
echo "risk:         ${AIDA_INTAKE_RISK:-medium}"        # the risk ceiling for the groom step
echo "max_approvals:${AIDA_INTAKE_MAX_APPROVALS:-∞}"   # cap on how many drafts you may approve
```

`AIDA_INTAKE_CANDIDATES` is **your fence** — the only specs you may act on. The
do-not-approve classes (vision / epic / principle / constraint / decision /
term) and any `needs-human` / `strategic` spec were **already excluded by the
launcher** — you will not see them in the fence, and you must never approve one
even if you find it some other way. If the fence is empty, there is nothing to
do; stop.

## The disposition bias (P1)

`AIDA_INTAKE_DISPOSITION_BIAS` tunes your worth-doing posture:

| Bias | Posture |
|------|---------|
| **`approve-eligible`** (default) | Propose approve for every spec in the fence that is clear and bounded. The propose-mode review is the worth-doing filter. |
| **`park-aligned`** | First read the operator's strategic frame — the project memory (`~/.claude/projects/<slug>/memory/`, especially `project_*` memories on the current phase / priorities) and `docs/plans/`. Propose approve only when a spec is BOTH eligible AND clearly aligned with that frame; otherwise park-for-human. |
| **`park-conservative`** | Park-when-unsure. No strategic-frame read; if a spec's value is not self-evident from its own text, park it. |

## Workflow

### 1. Read the fence

```bash
IFS=',' read -ra SPECS <<< "$AIDA_INTAKE_CANDIDATES"
for id in "${SPECS[@]}"; do aida show "$id" --card; done
```

Read each spec's title, description, and acceptance criteria. For
`park-aligned`, also read the strategic frame first (see the bias table).

### 2. Judge each spec — propose a disposition

For every spec in the fence, decide one of:

- **approve** — clear, bounded, worth doing now. (A *draft* you'd promote to
  Approved; an *already-Approved* spec you'd bless onto the queue.)
- **reject** — stale, duplicate, or superseded. Name what supersedes it.
- **park-for-human** — needs a human strategic call, a missing acceptance
  criterion, or a real design fork. **This is the safe default when unsure.**
- **queue** — already Approved and decision-free; belongs on the queue.

Honor the caps: never propose more than `AIDA_INTAKE_MAX_APPROVALS` approvals
(approve the highest-value ones first). Respect the risk ceiling — a spec
heavier than `AIDA_INTAKE_RISK` should be parked, not approved.

**Also classify HOW each approve/queue spec runs** (STORY-776 / ADR-13): pick
an execution mode — `drain` (full auto through merge), `drive` (auto through
CI, human merges), `guided` (interactive keystone dialog), `operator` (human
does the work), `decide` (a pending decision blocks). Keystone / architecture /
security / supervised tags force `guided` or `operator`; `human-only` forces
`operator`; a spec you'd park for a missing decision is `decide`. When unsure
between `drain` and `drive`, pick `drive` — the human keeps the merge. Carry a
short reason per mode call ("guided: carries keystone tag") so a wrong
classification is visible in review.

### 3. Output the proposal (always — this is the reviewable artifact)

Render a compact table, one row per spec: `SPEC-ID · disposition · mode ·
one-line reasoning`. Group by disposition. End with a count summary
(`N approve, M reject, K park, J queue`). This is the operator's review
surface — make the reasoning crisp enough to skim.

### 4. If `AIDA_INTAKE_APPLY` is not `1` — STOP HERE

Propose-mode writes nothing. Tell the operator to re-run with `--apply` to
execute, then end. **Do not edit any spec.**

### 5. Under `--apply` — execute, in order

Only when `AIDA_INTAKE_APPLY=1`:

1. **Approvals** — for each draft you proposed approve:
   ```bash
   aida edit <DRAFT-ID> --status approved --mode <MODE>
   ```
   The `--mode` is the execution-mode classification from step 2 (STORY-776) —
   writing it at bless time is what makes `aida do <spec>` dispatch with zero
   flags. Also write it for every **queue**-disposed spec that lacks one
   (`aida edit <ID> --mode <MODE>`). Never approve a spec you flagged
   park-for-human. **NEVER** approve a do-not-approve class or a
   `needs-human`/`strategic` spec (they are not in your fence; do not reach
   outside it).

2. **Rejections** — for each spec you proposed reject:
   ```bash
   aida comment add <ID> "intake: rejecting — <reason>"
   aida edit <ID> --status rejected
   ```

3. **Park-for-human** — leave the draft as-is, or route the open question to
   the decision inbox so it is parked + visible:
   ```bash
   aida questions ask <DRAFT-ID> ...      # pose the specific blocking question
   ```
   A parked draft stays out of the queue until a human answers it.

4. **Queue step (reuse, don't reimplement — acceptance #4)** — groom the
   decision-free Approved set onto the queue with the same gate the burndown
   uses:
   ```bash
   aida backlog groom --pickable --apply --risk "$AIDA_INTAKE_RISK"
   ```
   This picks up both the specs you just approved and any pre-approved-not-
   queued specs, applying the burndown pickability gate (bounded, unblocked,
   decision-free, not parking-tagged) + the risk ceiling. (Note: `--pickable`
   grooms the whole decision-free Approved set, so a pre-existing Approved spec
   outside this run's `--only-tag`/`--exclude-tag` may also be queued — it was
   already advisor-approved, so queuing it is legitimate. The tag filters scope
   what you newly *approve*, not the groom.)

5. **Drain (only if `AIDA_INTAKE_ON_APPLY=drain`)** — chain straight into a
   burndown over what you just queued:
   ```bash
   aida burndown run
   ```
   The default is `queue` — stop at queuing, leaving the drain a separate
   explicit step. This bounds the compounding unattended authority.

### 6. Post-run integrity report (BUG-496 lesson — acceptance #5)

After `--apply`, report exactly what changed: which specs you approved, which
you rejected, which you parked, and what landed on the queue
(`aida queue list`). If you chained a drain, note the burndown's exit and point
the operator at `aida findings list` for anything it shelved.

### 7. End

A headless `claude -p` run exits on its own. Make the report your last action.

## Two traps to watch

- **Every command here is a real shell line** — no invented flags. If you want
  a capability `aida` doesn't expose, file a TASK, don't fake it.
- **Do not reach outside the fence.** `AIDA_INTAKE_CANDIDATES` is the complete
  set you may act on. The launcher fenced out the strategic + knowledge-graph
  classes and the `needs-human`/`strategic` specs on purpose — that exclusion
  is the HARD authority bound, not a suggestion.

## Related skills / commands

- `aida intake` — the launcher that spawns this skill (propose-by-default;
  `--apply` executes; `[intake]` config tunes the policy).
- `/aida-backlog-groom` — the HUMAN-guided sibling (STORY-558): same surface,
  but the operator makes the approve call instead of a cold-boot agent.
- `/aida-burndown` — the implementer fan-out this pass feeds (the natural next
  step when `on_apply=queue`).
- `/aida-advise` — the per-punt headless advisor tier (resolves one design-fork
  mid-drain); intake is its batch read-all-open-specs cousin.

trace:STORY-560 | ai:claude
