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

## When to use

- Monthly cadence — first session of the month is a good default.
- After a notable change to the CLI surface (new subcommand, deprecation,
  reshuffle) — confirms whether usage followed the design intent.
- After a wave of orchestrator work — check whether drain success climbed.
- Before a release — surfaces UX-gap and calibration-gap follow-ups worth
  filing into the next iteration.

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
