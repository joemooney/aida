# AIDA Insights

Monthly usage-pattern review: most-used commands, drain success rate,
advisor calibration agreement %, and the deprecation / UX-gap /
substrate-gap follow-ups they suggest. Pass `--health` (or `--deep`) for
the on-demand health fact-finding mode.

## Mode

Arguments: `$ARGUMENTS`

- If `$ARGUMENTS` contains `--health` or `--deep`, run the **Health
  fact-finding mode** from `.claude/skills/aida-insights.md` (steps H1–H5):
  pull `aida usage --health --json` + `aida usage --auto-complete --json`,
  flag the loudest anomaly, root-cause it, bucket the free-text failure
  reasons, and synthesise a short health narrative + recommended fixes.
  Read-only; never auto-file, never set an automatic threshold.
- Otherwise, run the monthly synthesis below.

## Snapshot

!`aida usage --limit 10 2>/dev/null || echo "(no usage data — telemetry may be disabled)"`

!`aida usage --auto-complete 2>/dev/null || echo "(no auto-complete telemetry)"`

!`aida findings calibration --stats 2>/dev/null || echo "(no calibration data)"`

## Instructions

Follow the workflow in `.claude/skills/aida-insights.md`:

1. Read the three signals from the snapshot above — top 3 non-statusline
   commands, drain success rate (`N% success`), calibration agreement
   (`agreed N/M`). Lead with the headline numbers, don't recite tables.
2. Surface deprecation candidates (`aida usage --unused 30d`), UX-gap
   candidates (`aida usage --errors`), orchestrator-fix candidates
   (`aida usage --auto-complete --pattern`), and substrate-gap candidates
   (`aida findings calibration --disagreement`) as discrete follow-ups.
3. Offer the obvious next moves: drill into one signal, file the loudest
   finding as a TASK/BUG, or stop.
4. If `paired: 0` in calibration stats, point out that calibration mode
   is off (`[advisor] calibration_mode = "on"` in `.aida/config.toml`)
   rather than reporting "100% agreement" off zero samples.
5. Read-only. Every suggestion is a recommendation, never auto-run.

For the on-demand health fact-finding mode (when `$ARGUMENTS` carries
`--health` / `--deep`), follow steps H1–H5 in the "Health fact-finding
mode" section of `.claude/skills/aida-insights.md`.
