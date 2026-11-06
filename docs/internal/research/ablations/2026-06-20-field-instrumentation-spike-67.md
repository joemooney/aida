# Field instrumentation of stated-rule adherence (SPIKE-67, slice 1)

**Date:** 2026-06-20
**Spec:** SPIKE-67 (child of EPIC-50; continues the gate-vs-rule program, STORY-655 terminus)
**Status:** slice 1 shipped — the sensor + query surface. Harvest is ongoing.

## Why this exists

The gate-vs-rule ablation program (`2026-06-17-gate-vs-rule.md` … `-i4.md`) reached a
methodological terminus: five controlled cells, all 100% rule-only / 0 gate-saves. A
**clean ablation cannot reproduce rule-dropping at all** — the lone observed drop (the
bake-off, n=1) lives in a regime controlled designs cannot reach: a large real codebase
under long-horizon autonomy. The only remaining instrument is **field telemetry**: stop
trying to manufacture the violation in a lab, and instead measure adherence in real use.

The paper named field instrumentation as the only open P1 evidence path. This is its
first slice.

## The design: the git log *is* the planted sensor

The obvious implementation — a live git-hook on every commit, or a hook inside the
`--auto-complete` orchestrator — was **rejected for slice 1**:

- A universal commit-msg hook call ships a speculative per-commit subprocess to *every*
  downstream AIDA project for a study that may not pan out (blast radius).
- The orchestrator is keystone autonomy machinery — instrumenting it unattended is
  exactly the recursive-failure risk we avoid (reliability fixes ride the keyboard).

Instead: **every commit already records its message + diff.** The git log is a sensor
that was planted the day the repo started. `aida field-study scan` recomputes the
stated-rule verdicts over recent commits and appends one observation per (commit, rule)
to a local-only log; `aida field-study report` aggregates it. No hook, no orchestrator
edit, real data available immediately (months of history to harvest), and the same code
path will work against any downstream repo's history later.

Two rules are evaluated per commit:

- **`commit_format`** — would the commit-msg hook's validator (`commit::validate_message`,
  the in-Rust mirror of `aida-core/templates/hooks/aida-commit-msg`) have rejected the
  subject? (conventional format / feat-fix needs a REQ-ID / AI-traced needs `[AI:tool]`.)
- **`trace_presence`** — the commit changed code files but carries *no* `trace:` comment
  at all. This rule is **not enforced anywhere today**, so every miss currently slips
  through — making it a pure observe-only adherence signal.

Merge commits are excluded (synthetic subjects, no authored diff).

## Privacy floor

Identical posture to the usage log. An observation carries only: scan timestamp, commit
short-SHA (a public git identifier), rule name, the boolean verdict, an optional single
inferred SPEC-ID (an identifier breadcrumb, never requirement content), the count of
changed code files (the task-span proxy), and a coarse repo-size bucket. **No commit
message text, no file paths, no diff content** — the `RuleObservation` struct has no
field that could carry them (enforced by a unit test).

Opt-IN, default OFF: `AIDA_FIELD_STUDY=1` or `[field_study] enabled = true`. Honors the
global `AIDA_TELEMETRY=0` kill-switch. Local-only; never phoned home.

## First harvest (this repo, 150 commits, 2026-06-20)

| rule | would-block overall | by task span (code files changed) |
| --- | --- | --- |
| `commit_format` | 92/150 (61%) | 0f: 8% · 1f: 68% · 2-3f: 87% · 4-9f: 95% · 10+f: 50% |
| `trace_presence` | 3/110 (3%) | violations concentrated in 1-file commits |

The headline signal is exactly the residual the controlled ablations could not reach:
the `commit_format` would-block rate **rises monotonically with task span** (8% → 68% →
87% → 95% across the 0→4-9 file buckets). That is the shape SPIKE-67's hypothesis
predicted — adherence degrades as the change grows.

### Honest caveats (this is slice-1 data, not a verdict)

- **Confound: span correlates with commit *type*.** Bigger commits are more likely
  feat/fix (which the strict rule requires a REQ-ID for); tiny commits are more likely
  docs/chore (which pass trivially). Part of the rising curve is "more commits in the big
  buckets are the kind the rule applies to," not purely "the rule gets dropped under
  load." Disentangling type from span is the next analysis cut.
- **Historical mix.** The window includes commits authored before the discipline was
  enforced, so the absolute rate is not "how often agents drop the rule *now*."
- **No drain/headless attribution yet.** Slice 1 cannot yet tag a commit as
  drain-produced vs interactive, or headless vs supervised — the very axes SPIKE-67 most
  wants. That correlation (join against `~/.aida/auto-complete.jsonl` run windows) is the
  highest-value follow-up.

## How to run

```bash
export AIDA_FIELD_STUDY=1
aida field-study scan --limit 200      # idempotent; re-running adds only new commits
aida field-study report                # adherence by rule, bucketed by task span
aida field-study report --json         # machine-readable
```

## Follow-ups (filed as children of SPIKE-67 / EPIC-50)

- Drain-vs-interactive + headless-vs-supervised attribution (join with
  `auto-complete.jsonl`) — the attribution that turns "span" into "context pressure."
- Type-vs-span disentanglement in the report (control for feat/fix).
- Cross-repo harvest: run the scanner against larger external repos to populate the
  repo-size buckets that a single repo's history can't.
- *If* the field signal proves worth a live sensor: a best-effort, opt-in commit-msg hook
  observe call (still no orchestrator edit).

trace:SPIKE-67
