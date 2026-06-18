# aida health polish: severity-order the issue list

Date: 2026-06-18 · Specs: TASK-853 (builds on STORY-658) · Status: Implemented · Complexity: XS (~30 lines)

## Approach

From the SPIKE-64 run-2 synthesis, the Claude `aida health` arm shipped as-is and we
graft exactly one thing from the Codex arm: **order the issue list by severity (worst
first) before printing**, plus the optional polish of a **dimmed detail line** under each
non-healthy vital. We deliberately do **not** take Codex's 0-100 score or its duplicate
lease/queue readers (the regression the bake-off flagged).

The change lives in two pure spots. `HealthReport::build` already extends + rolls up the
worst-anchored overall grade over the combined vital list; we add one stable
`sort_by_key(Reverse(grade))` there so the `vitals` vec is worst-first for every consumer
(human view *and* `--json`). Stability preserves the original per-axis definition order
within a single grade. The human renderer groups by axis with a filter, so each axis
section inherits the worst-first order for free — no second sort. The renderer also now
prints the existing `Vital.detail` (previously JSON-only) as a dimmed line for non-healthy
vitals, giving inline context; healthy readings stay quiet.

```
backlog_vitals + coordination_vitals
        │  extend into one Vec<Vital>
        ▼
   overall = max(grade)              (worst-anchored rollup, unchanged)
        │
        ▼
   sort_by_key(Reverse(grade))       ← NEW: stable, worst-first
        │
        ├── --json ──► vitals already worst-first
        └── human  ──► filter by axis (order preserved)
                         per vital: label+value
                         if !Healthy: dimmed detail line  ← NEW
                         if remedy:   dimmed ↳ remedy      (unchanged)
```

## Decisions

- **Sort in `build()`, not at print time.** Keeps the ordering in one pure, unit-testable
  place and makes JSON consistent with the human view. The axis-grouped renderer needs no
  change to ordering because a stable sort preserves within-axis relative order.
- **Sort the whole vec by grade only (not axis-then-grade).** The renderer re-separates by
  axis via `filter`, so a global grade sort yields worst-first *within* each axis section
  while also making the flat `--json` list worst-first. Simpler and one sort.
- **`Reverse(grade)` over the existing `Ord`.** `Grade` already derives `Ord` as
  `Healthy < Watch < Critical` (relied on by the `max()` rollup); `Reverse` reuses that
  rather than introducing a parallel severity rank.
- **Detail line only for non-healthy vitals.** Honest-not-noisy: a green read stays a
  single line; an issue gets its meaning inline above the remedy. Reuses the existing
  `Vital.detail` field (already populated, previously human-invisible) — no new data, so
  none of Codex's extra readers are pulled in.

## Files (in build-order)

1. `aida-cli/src/health.rs`
   - `HealthReport::build` — add `backlog.sort_by_key(|v| std::cmp::Reverse(v.grade));`
     after the `overall` rollup, before constructing the report.
   - `mod tests` — add `vitals_are_severity_ordered_worst_first`.
2. `aida-cli/src/main.rs`
   - `fn handle_health_vitals_command` — in the per-vital render loop, emit
     `println!("      {}", v.detail.dimmed())` when `v.grade != Grade::Healthy`, just
     before the existing remedy line.

## Critical Files

- `aida-cli/src/health.rs`
- `aida-cli/src/main.rs`

## Reusable helpers

- `health::Grade` (`PartialOrd`/`Ord` already derived) — drives both the `max()` rollup and
  the new `Reverse` sort; do not add a parallel severity rank.
- `health::HealthReport::build` / `counts` / `headline` — unchanged contract; the sort is
  additive.
- `health::Vital.detail` — existing field; surfaced in the human view instead of a new one.
- `crate::glyph` / `glyphs::Glyph::SubArrow` + `.dimmed()` — existing remedy-line styling,
  reused for the detail line.

## Risks + gotchas

1. **`sort_by_key` stability assumption.** `slice::sort_by_key` is documented stable, so
   within-grade per-axis order is preserved. *Mitigation:* test asserts Critical backlog
   vitals stay in definition order (`stale_specs` before `blocked_specs`).
2. **JSON consumers seeing a new order.** The `--json` `vitals` array is now worst-first.
   *Mitigation:* no test or downstream pins array order; worst-first is the more useful
   contract and is documented in the code comment.
3. **Detail-line noise.** Adding a line per non-healthy vital lengthens output when many
   things are red. *Mitigation:* gated on `!= Healthy` so a healthy project is unchanged;
   matches the existing remedy-line discipline.

## Tests

- `health::tests::vitals_are_severity_ordered_worst_first` — builds a mixed-grade report
  across both axes; asserts the full vital list is non-increasing in severity, the lead
  vital matches `overall`, and within-grade order is stable.
- Existing `one_critical_anchors_overall_critical` / `counts_sum_to_total` still pass
  (order-independent `find`/`any`/`counts`), confirming the sort doesn't hide vitals.

## Verification

```bash
cargo build && env -u AIDA_SESSION_ROLE cargo test -p aida-cli health
# positive: issue list is worst-first within each axis, dimmed detail under non-healthy
./target/debug/aida health
# machine check: --json vitals are globally non-increasing in severity
./target/debug/aida health --json | python3 -c '
import json,sys
rank={"healthy":0,"watch":1,"critical":2}
g=[rank[v["grade"]] for v in json.load(sys.stdin)["vitals"]]
assert g==sorted(g,reverse=True), g
print("OK worst-first")'
# negative: a healthy vital prints no detail/remedy line (output stays quiet)
```

## Followups

- TASK: consider a `--sort axis|severity` flag if anyone wants the original axis-order back.

## Related

- Builds on: STORY-658 (aida health vital-signs read)
- See also: SPIKE-64 run-2 synthesis (bake-off: ship-Claude-as-is + this graft)
