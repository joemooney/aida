---
name: aida-insights
description: Monthly usage-pattern review — most-used commands, drain success rate, advisor calibration agreement %, and the deprecation / UX-gap / substrate-gap follow-ups they suggest. Read-only; the "where is the project spending its attention?" surface.
allowed-tools:
  - Bash
---

# AIDA Insights Skill

## Purpose

Surface the three top-line signals telemetry already records and turn them
into concrete next moves:

1. **Per-command usage** (`aida usage`) — which `aida` subcommands the
   operator actually runs, which never fire, which fail most often.
2. **Drain reliability** (`aida usage --auto-complete`) — the orchestrator's
   success rate over the last 30d and which phase tends to break.
3. **Advisor calibration** (`aida findings calibration --stats`) — the
   rolling agreement rate between cold-boot and fork-from-live advisor
   verdicts, the signal for substrate gaps.

Run monthly. The cadence is the point: command-mix shifts, drain-failure
patterns, and calibration drift only show up when you compare today's
numbers to last month's gut feel.

There is also an **on-demand health fact-finding mode** (invoke with a
`--health` / `--deep` argument): when a deterministic health metric looks
anomalous, it puts you (the agent) to work root-causing the anomaly,
bucketing the unstructured failure reasons, and synthesising a short health
narrative with recommended fixes. It runs **only** when the operator asks
for it — never on the monthly cadence, never automatically. See "Health
fact-finding mode" below.

## When to use

- Monthly cadence — first session of the month is a good default.
- After a notable change to the CLI surface (new subcommand, deprecation,
  reshuffle) — confirms whether usage followed the design intent.
- After a wave of orchestrator work — check whether drain success climbed.
- Before a release — surfaces UX-gap and calibration-gap follow-ups worth
  filing into the next iteration.
- **On demand, when the orchestrator is misbehaving** — invoke with
  `--health` (or `--deep`) to have the agent root-cause the loudest
  health anomaly rather than just report the numbers.

## Skip if

- The operator wants a single number — point them at the underlying command
  (`aida usage`, `aida usage --auto-complete`, `aida findings calibration
  --stats`) directly. This skill is the synthesis layer.
- Telemetry is disabled (`AIDA_TELEMETRY=0` or `[telemetry] enabled =
  false`) — there is nothing to read. Surface that as the finding.
- The operator asked for project status or a digest — `/aida-status` and
  `/aida-digest` are the right surfaces; this skill is narrower (telemetry
  patterns only).

## Snapshot

!`aida usage --limit 10 2>/dev/null || echo "(no usage data — telemetry may be disabled)"`

!`aida usage --auto-complete 2>/dev/null || echo "(no auto-complete telemetry)"`

!`aida findings calibration --stats 2>/dev/null || echo "(no calibration data)"`

## Workflow

### Step 1: Read the three signals

The snapshot above runs the three commands. Lead with the headline number
from each — don't recite the full tables.

- **Most-used**: top 3 commands by count. If `statusline` dwarfs everything
  else (it usually does), name the top 3 *non-statusline* commands — that
  is where the operator is actually spending intent.
- **Drain success rate**: the `(N% success)` figure from
  `aida usage --auto-complete`. <50% means the orchestrator is the
  bottleneck; >80% means it's reliable infrastructure.
- **Calibration agreement %**: the `agreed N/M` figure from `--stats`.
  Low agreement names substrate gaps; `paired: 0` means calibration mode
  is off (turn it on via `[advisor] calibration_mode = "on"` in
  `.aida/config.toml`).

### Step 2: Surface deprecation / UX-gap / substrate-gap candidates

Each signal points at one kind of follow-up:

- **Deprecation candidates** — commands not used in 30/60/90 days:

  ```bash
  aida usage --unused 30d
  aida usage --unused 90d
  ```

  Long-unused subcommands are de-facto dead surface. Worth filing a TASK
  to evaluate whether the command earns its place or should be folded
  into another verb.

- **UX-gap candidates** — high error rate over total invocations:

  ```bash
  aida usage --errors
  ```

  A subcommand with >20% error rate is either confusingly designed or has
  a missing flag pattern. Each row is a candidate for a UX TASK
  (per `feedback_failed_flag_attempts_are_ux_signals`).

- **Orchestrator-fix candidates** — which phase fails most:

  ```bash
  aida usage --auto-complete --pattern
  aida usage --auto-complete --failures
  ```

  The `--pattern` view names the phase to invest in; `--failures` lists
  every recent failure with its drafted BUG (if any).

- **Substrate-gap candidates** — calibration disagreements:

  ```bash
  aida findings calibration --disagreement
  ```

  Each disagreement is a substrate-gap signal — the cold-boot advisor
  reached a different verdict than the warm one, which usually means the
  warm one's context is *not* in writing yet. Worth promoting the
  highest-frequency gaps to memories (annotate with
  `aida findings calibration annotate <punt-id> "gap → wrote memory
  <name>"`).

### Step 3: Offer next moves

After the synthesis, offer the deprecation/UX/orchestrator/substrate
threads as discrete follow-ups, not a forced sequence:

| Path | What it answers | Command |
|------|-----------------|---------|
| ▶ Drill into deprecation | "Which commands earn their place?" | `aida usage --unused 60d` |
| ▶ Drill into UX gaps | "Which commands confuse the operator most?" | `aida usage --errors` |
| ▶ Drill into orchestrator | "Which drain phase is the bottleneck?" | `aida usage --auto-complete --pattern` |
| ▶ Drill into substrate | "Where is the advisor's context not yet written?" | `aida findings calibration --disagreement` |
| ⇒ File the findings | Capture the loudest signal as a TASK / BUG | `aida add --title "..." --type task` |
| ⏸ Stop | The snapshot landed in scrollback; nothing else required | — |

## Health fact-finding mode (on demand)

The monthly synthesis above is read-and-report. This mode is read-and-
**reason**: when a deterministic project-health metric looks anomalous, you
(the agent) do the work the numbers alone can't — root-cause the anomaly,
bucket the free-text failure reasons, and write a short narrative with
recommended fixes.

**Invocation is explicit.** Run this mode only when the operator invokes the
skill with a `--health` or `--deep` argument (parse it from `$ARGUMENTS`):

- `$ARGUMENTS` contains `--health` or `--deep` → run the workflow below.
- `$ARGUMENTS` is empty or anything else → stay on the monthly synthesis
  above; do **not** auto-run this mode.

This is deliberately manual: it spends a real LLM turn reasoning over logs,
so it fires on request, never on a schedule. (Automatic anomaly thresholds
and cost controls are intentionally out of scope — a future iteration.)

### Step H1: Pull the deterministic catalog as JSON

```bash
aida usage --health --json
aida usage --auto-complete --json
```

The first is the Tier-1 catalog; the second carries the session-vs-drain
misclassification gap. Parse both. The catalog object has:

- `phase_failure_distribution` — `[{phase, phase_slug, failures}]`: which
  lifecycle phase the drain breaks on most.
- `reap_vs_kill` — `{total, success_rate, breakdown:[{outcome, count,
  counts_as_success}]}`: how headless sessions ended (clean-success /
  sentinel-reaped count as success; mid-work-kill / error / truncated don't).
- `drain_halt_rate` — `{shelved, halted, unclassified, halt_rate}`: how often
  a drain parks-and-continues (shelved) vs stops the whole batch (halted).
- `recovery_latency` — `{count, mean_secs, median_secs, max_secs}`: how long
  parked work sat before the next drain (the babysitting cost).
- `draft_inbox_depth` — untriaged Draft specs awaiting an approve/reject.
- `burn_down_velocity` — `{completed, added, days, net, net_per_day}`:
  positive net = backlog shrinking, negative = adding faster than shipping.

The `--auto-complete --json` object carries a `gap` block:
`{session_success_rate, drain_success_rate, gap, session_total, drain_total,
insufficient_data, session_breakdown}`. The `gap` is
`session_success_rate − drain_success_rate`; a positive gap is the
**orchestrator misclassification rate** — work the session actually finished
but the drain scored as a failure.

### Step H2: Flag what looks anomalous

You are the threshold here — there is no auto-rule. Judge each metric in
context and pick the **one or two loudest** anomalies, not all of them.
Rough reading guide (sanity, not law):

- **Misclassification gap** (`gap`) materially positive (e.g. ≳ 15%) →
  the orchestrator is scoring real successes as failures. Loud.
- **Phase-failure distribution** concentrated on one phase → that phase is
  the bottleneck.
- **Halt-rate** elevated (`halted` > 0, especially `halt_rate` high) → the
  drain is hitting broken-environment failures (`spawn` / `missing-tool` /
  `internal`) that stop the batch instead of degrading gracefully.
- **Recovery latency** large `max_secs` / `mean_secs` → work parks and sits;
  the babysitting cost is high.
- **Draft-inbox depth** climbing → unreviewed backlog piling up.
- **Burn-down velocity** negative `net_per_day` → adding faster than shipping.

If `insufficient_data` is true, or every count is zero, say so plainly and
stop — there's nothing to root-cause yet.

### Step H3: Root-cause the loudest anomaly

For the anomaly you picked, go past the number to the mechanism:

- **Gap is N% positive** → read the recent drain failures and the headless
  session outcomes that disagree with them, then name the *likely
  orchestrator misclassification bug* (e.g. "the session ended clean but the
  drain scored phase-3 as RequestChanges — the reviewer verdict parser is
  treating an advisory comment as a block"). Pull the evidence:

  ```bash
  aida usage --auto-complete --failures
  ```

  Each row carries the failed phase, the `failure_kind`, and the free-text
  `failure_message`. Cross-read those against the `session_breakdown` from
  Step H1 (clean-success / sentinel-reaped sessions that still produced a
  drain failure are the misclassified ones).

- **One phase dominates** → name that phase (`phase_slug`) and the most
  common `failure_kind` on it; that pair is the thing to fix.

- **Halt-rate elevated** → list the halting `failure_kind`s
  (`spawn`/`missing-tool`/`internal`) and what environment gap each implies.

### Step H4: Bucket the unstructured failure reasons

The `failure_message` field is free text. Group the recent failures into a
handful of named categories so the long tail becomes a short list. Read
them with:

```bash
aida usage --auto-complete --failures
```

Then bucket by *cause*, not by wording — e.g. **CI-flake** (transient test /
network), **review-block** (reviewer requested changes), **build-break**
(compile / fmt / clippy), **environment** (spawn / missing-tool / internal),
**verdict-ambiguous** (orchestrator couldn't read a clear verdict). Report
each bucket with its count and one representative message. A bucket that is
mostly *verdict-ambiguous* or *review-block on clean sessions* is itself a
misclassification signal feeding back into Step H3.

### Step H5: Synthesise the health narrative + fixes

Close with a short, plain narrative (a few sentences, not a table dump):

1. **Headline** — the single most important health fact this run
   ("orchestrator is misclassifying ~1 in 5 successful drains as failures").
2. **Root cause** — the mechanism you named in Step H3.
3. **Buckets** — the failure-reason categories from Step H4, with counts.
4. **Recommended fixes** — concrete, smallest-first. Offer to file the
   loudest one as a BUG/TASK (a recommendation, never auto-filed):

   ```bash
   aida add --title "..." --type bug --status approved
   ```

Keep the narrative free of internal spec identifiers — those belong in the
filed BUG/TASK and commit trailers, not in the operator-facing report.

## Telemetry-off case

If the snapshot is empty (`no usage data`), surface that as the finding:

> Telemetry is disabled — `~/.aida/usage.jsonl` does not exist. To turn it
> back on, unset `AIDA_TELEMETRY` and set `[telemetry] enabled = true` in
> `.aida/config.toml`. Privacy floor: only command shapes are logged, never
> argument values or file paths.

## Notes

- Read-only. This skill never mutates state — every suggestion (file a
  TASK, write a memory, turn on calibration) is surfaced as a recommendation
  for the operator.
- Calibration agreement is only meaningful when `[advisor] calibration_mode
  = "on"`; if `paired: 0`, point that out rather than reporting "100%
  agreement" off zero samples.
- Sibling surfaces — `/aida-digest` (narrative work report),
  `/aida-status` (one-shot project state), `/aida-doctor` (substrate drift)
  — answer different questions; insights is the *telemetry-pattern* lens.
- The health fact-finding mode (`--health` / `--deep`) is the one part of
  this skill that reasons rather than reports — but it is still read-only and
  still on-demand only. It never sets a threshold and acts on its own; every
  recommended fix is surfaced for the operator to file, not auto-filed.
