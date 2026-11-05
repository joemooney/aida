# Field study slice 2 — vendor / drain / type controls (SPIKE-67)

**Date:** 2026-06-22
**Spec:** TASK-891 (child of SPIKE-67 → EPIC-50; multi-vendor angle feeds EPIC-48)
**Status:** slice 2 shipped — the three attribution controls + a methodology correction.
**Builds on:** `2026-06-20-field-instrumentation-spike-67.md` (slice 1 — the sensor itself).

## What slice 2 adds

Slice 1 recorded a per-commit would-block verdict bucketed by task span and reported one
headline: the `commit_format` would-block rate **rose monotonically with span** (8% → 95%).
Slice 1 named its own confounds and could not yet cut the data along the axes SPIKE-67 most
wants. Slice 2 adds three controls to `aida field-study report` and the `--json` payload:

- **(a) vendor** — parse the `[AI:tool]` subject tag into a `vendor` field per observation
  (`claude`, `codex`, `antigravity`, `antigravity+claude`, …; `untagged` = human). Report the
  would-block rate and the span trend **per vendor**. The question (EPIC-48): *do prose rules
  port across vendors, or does adherence-under-load differ by vendor?*
- **(b) drain-vs-interactive** — join each observation's inferred SPEC-ID against the set of
  specs that appear in `~/.aida/auto-complete.jsonl` (the autonomous-drain run log). A spec the
  orchestrator ever drained → `drained`; a spec'd commit not in that set → `interactive`; a
  commit with no inferred spec → `unattributed`. This is the context-pressure axis.
- **(c) type control** — parse the conventional-commit type into a `commit_type` field and
  report the span trend **over all commits vs over feat/fix-only commits**. If the all-types
  curve rises but the feat/fix-only curve goes flat, the "span effect" was commit-*type*
  composition masquerading as span.

Each control prints a mechanical `rises` / `flat` / `falls` verdict (≥15-point swing between the
smallest- and largest-span populated buckets). **`flat` is a valid null result** — the
acceptance treats "the effect washes out under the control" as reportable probe data.

## The methodology correction (the most important slice-2 finding)

Building the type control surfaced a confound that **slice 1's headline did not account for and
that materially inflated it**: the sensor reads `main`, whose commits are **GitHub squash-merges**.
A squash-merge appends ` (#NNNN)` to the subject:

```
authored:  [AI:claude] fix(init): drop double-verb (BUG-604)          → PASSES the validator
on main:   [AI:claude] fix(init): drop double-verb (BUG-604) (#1079)  → FALSE "missing (REQ-ID)"
```

The commit-format validator anchors the REQ-ID at end-of-line (`(REQ-ID)$`). The appended
`(#1079)` shoves the real `(BUG-604)` out of that position, so the sensor scores a `feat`/`fix`
the agent authored **correctly** (and which passed the local commit-msg hook *before* the
squash) as a rule miss. The local hook is never wrong here — it runs pre-squash and never sees
the rewritten subject; only the field sensor, re-evaluating `main`, does.

**Fix (study-local, scanner-only): strip a trailing ` (#NNNN)` before evaluating
`commit_format`**, reconstructing the authoring-time subject. `validate_message` — the real hook
— is deliberately left untouched (loosening it would let a genuinely REQ-ID-less `feat` through
at commit time). The strip never masks a real miss: a subject that lacks a `(REQ-ID)` even after
stripping the PR number still fails.

The size of the artifact, this repo, 750 commits (HEAD~750):

| measure | pre-strip | post-strip |
| --- | --- | --- |
| `commit_format` overall would-block | 570/1000 (57%) | 176/750 (23%) |
| feat/fix-only span trend | flat @ ~84% | flat (28% → 40%) |

The squash suffix accounted for **~3 of every 5** apparent `commit_format` misses. Slice 1's
"8% → 95%" curve was inflated by it on top of the type confound slice 1 already flagged.

## Results after the correction (this repo, 750 commits, 2026-06-22)

### `commit_format`

| cut | result |
| --- | --- |
| overall | 176/750 (23%) would-block |
| span, all types | **rises** (0f: 10% → 10+f: 44%) |
| span, feat/fix only | **flat** (0f: 28% → 10+f: 40%) — the span effect washes out |
| vendor | codex 1% · claude 21% · antigravity 21% · untagged 40% |
| drain | **drained 2%** (2/85) · interactive 17% · unattributed 30% |

**Does a span/load effect survive each control?**

- **Type control: NO — it washes out.** The apparent span effect is mostly commit-*type*
  composition: the 0-file bucket is docs/chore (passes trivially), the bigger buckets are
  feat/fix (carry the strict REQ-ID requirement). Hold type to feat/fix and span goes flat
  (12-point swing, under the threshold). This confirms slice 1's stated suspicion with data.
- **Drain control: NO — it inverts.** Drained (orchestrator-produced) commits adhere *better*
  (2% would-block) than interactive (17%). The autonomous path runs `aida commit` with proper
  trailers, so context pressure does **not** degrade `commit_format` adherence here — if
  anything the disciplined harness improves it. (Pre-strip this axis read 75% vs 60% — an
  artifact of the squash suffix, since merged-PR commits are exactly the ones carrying `(#NNNN)`.
  The correction reversed the conclusion; a cautionary tale about reading the raw rate.)
- **Vendor control: differs, but confounded.** codex commits almost never trip `commit_format`
  (1%); untagged/human commits trip it most (40%). The spread is real but co-varies with the
  type/era mix each vendor worked, so it is suggestive, not a clean vendor effect. Per-vendor
  span trends mostly `rise`, but on small tail-bucket n.

### `trace_presence`

Overall 178/563 (32%) would-block; span **falls/flat** (more files → more likely to carry a
trace). **The drain-vs-interactive join is degenerate for this rule** and must not be read as a
signal: the join key (an inferred SPEC-ID) is the *same* trace whose absence the rule measures,
so a trace-missing commit has no spec to join on — every miss lands in `unattributed` (178/323)
and `drained`/`interactive` are structurally 0%. Vendor and span cuts remain valid for this rule;
the drain cut does not apply.

## Net read for the paper

The field signal that motivated SPIKE-67 — "prose rules measurably fail under load" — **does not
survive the controls in this repo's history.** Once the squash artifact is removed and commit
type is held fixed, the span effect is flat, and the autonomy/drain axis shows the disciplined
harness adhering *better*, not worse. This is a **null result, and a valid one**: lab (the
STORY-655 ablations: 100% rule-only, 0 gate-saves) and field now **agree** that a clean signal of
prose-rule-dropping under load is not reproducible here. That convergence is itself probe data —
it sharpens the moat argument toward *"the substrate gate's value is uniform enforcement and
vendor-neutrality, not patching a measurable prose-rule failure rate."*

Caveats that keep this honest (and bound what the null result claims):

- **Single repo, disciplined operator.** Absence of a measurable failure here ≠ absence
  everywhere. The cross-repo harvest (run the scanner against larger, less-disciplined external
  histories) is the way to populate the regime where slice 1 expected the effect to live.
- **Spec-level, not session-level, drain attribution.** The join marks a spec the orchestrator
  *ever* drained; it cannot tell which individual commit a drain produced. **Headless-vs-supervised
  is not separable at all** — `auto-complete.jsonl` records no human-mode flag.
- **Historical mix.** The 750-commit window predates parts of the current discipline; absolute
  rates are not "how often agents drop the rule *now*."

## How to run

```bash
export AIDA_FIELD_STUDY=1
aida field-study scan --since HEAD~750 --limit 750   # deep enough to reach drained specs
aida field-study report                              # overall + by-span + vendor/drain/type controls
aida field-study report --json                       # adds by_vendor / by_drain / type_control
```

To backfill the new attribution onto a pre-slice-2 log, delete `~/.aida/field-study.jsonl` and
re-scan (the scanner is idempotent by `(sha, rule)`, so it will not re-derive vendor/type onto
rows already on file).

## Follow-ups

- Cross-repo harvest against larger external histories (the regime a single disciplined repo
  can't reach) — the only way to test whether the null result generalizes.
- *If* a future harvest shows a surviving effect: a best-effort, opt-in commit-msg-hook observe
  call (still no orchestrator edit), to capture authoring-time subjects directly and sidestep the
  squash-suffix reconstruction entirely.

trace:SPIKE-67 trace:TASK-891
