# CLI reference manual — rationale + lifecycle narrative, drift-guarded

**Date:** 2026-06-12 · **Specs:** EPIC-40 + per-chapter children · **Status:** Proposed · **Complexity:** Medium-High (large doc surface, low code risk; one small drift-guard tool)

> Operator ask: a manual that goes deeper than `--help` — when/why every command & option should (and should not) be used — woven into a story that educates users about the whole lifecycle. `lifecycle.md` is the seed; this is far more comprehensive.

## 1. Approach

**The core decision is architectural, not editorial:** a hand-typed manual covering 78 commands × N options will drift from the binary within days (the panel already caught us on "40 skills" vs 45). So:

- **Structure = `aida help-all`'s 11 chapter-groups** — the binary already maintains the grouping; the manual mirrors it, so the two never disagree on *which* commands exist.
- **The manual owns *rationale*; `--help` owns *facts*.** We never reproduce full flag lists or defaults — only options whose *why* isn't obvious. Rationale is stable even when a flag name changes; facts are not.
- **A narrative spine** (the journey, in `docs/cli/README.md`) carries the educational "what to expect" story and links into the per-command reference.
- **A drift-guard** (a small CI-wired gate) asserts every `help-all` command has a manual entry and warns when an entry's `--help` changed since last touched. The manual may lag on prose; it cannot silently *omit* a command.

**Per-command entry template** (fixed by the exemplar, `docs/cli/01-getting-started.md`):
One-line · Mental model · Reach for it when · Don't reach for it when (+ what to use instead) · Key options (rationale only) · Gotchas · Chains with.

```
docs/cli/
  README.md            ← index + the lifecycle JOURNEY + how-to-read + drift-guard note
  01-getting-started.md  ← EXEMPLAR (init/add/list/show/done/edit) — the quality bar
  02-specs.md … 11-dev.md  ← one file per help-all group
  _completeness.<test>   ← the drift-guard
```

## 2. Decisions

- **D1 — Mirror `help-all`, don't invent a taxonomy.** The binary's grouping is the source of structure; if a command moves groups in `help-all`, the manual follows.
- **D2 — Rationale-only; never copy `--help`.** Hard rule. The drift-guard enforces presence, not prose-completeness, precisely so we're never tempted to paste flag tables.
- **D3 — Ground every entry against live `--help` before writing.** No invented flags (the recurring doc-drift failure). Each chapter slice re-checks its commands.
- **D4 — Narrative + reference, cross-linked.** The journey is the on-ramp; the chapters are the depth. Neither alone is the manual.
- **D5 — Branch-isolated build.** Lands on `cli-manual` (worktree); merges to main when ≥ the exemplar + 2-3 chapters read coherently (a half-manual on main is worse than none for a reviewer). The README table's ✅/⬜ status column makes WIP honest if it does land partial.
- **D6 — This is stabilization-supporting, not a pivot.** Writing the manual *surfaces* CLI inconsistencies (overlapping commands, confusing flags, wrong help text) → those become bug/papercut filings. Expect the chapters to generate stabilization work as a side effect — that's a feature.

## 3. Files (build order)

1. `docs/cli/README.md` ✅ — index + journey spine.
2. `docs/cli/01-getting-started.md` ✅ — exemplar chapter.
3. `docs/cli/02-specs.md` … `11-dev.md` — one slice each (EPIC-40 children).
4. `docs/cli/_completeness` drift-guard (script + CI wire).
5. Pointer from root `README.md` / `OVERVIEW.md` to `docs/cli/` once ≥3 chapters land (reviewer visibility).

## 4. Critical files

- `01-getting-started.md` — the template; every other chapter copies its shape and depth. Get the bar right here.
- `README.md` journey — the one place a confused new user gets oriented; it must read as a *story*, not a TOC.
- the drift-guard — what turns a one-time doc into a maintainable one.

## 5. Reusable helpers (don't reimplement)

- `aida help-all` — the maintained grouping + inventory (the skeleton; don't re-curate it).
- `aida <cmd> --help` — the fact source each entry grounds against.
- `docs/lifecycle.md` — the state-machine reference the journey links to (don't duplicate the state machine; link it).
- `docs/aida/discipline/` — the vocabulary (lifecycle-vocabulary, machinery-glossary); the manual uses these terms, doesn't redefine them.

## 6. Risks + gotchas

- **Drift** (the headline risk) — mitigated by D2 + the drift-guard. The manual must never become a second source of truth for facts.
- **Slop from bulk generation** — writing 10 chapters fast = shallow, wrong-flag prose. Mitigation: one chapter per slice, each grounded against live `--help`, each matching the exemplar's depth; review chapter-by-chapter, not in a batch.
- **The autonomy chapter (Ch.3) is the hard one** — queue/backlog/burndown/drain/away/home/human/headless/no-human overlap heavily; it needs a *decision tree* ("you want to drain work and you're at the keyboard → X; away → Y; one spec → Z"), not just N entries. Budget it extra.
- **Half-manual on main** — D5 (branch until coherent; status column if partial).
- **Stabilization context** — this runs *alongside* bug-clearing; it's docs (low risk) and actively feeds stabilization (D6). Not tagged deferred.

## 7. Tests (named)

- `cli_manual_completeness` — every `aida help-all` command appears in some `docs/cli/*.md` (the drift-guard; fails on omission).
- `cli_manual_no_invented_flags` (optional/advisory) — flag tokens cited in a chapter that don't appear in that command's `--help` are warned (catches drift the other direction).
- Docs: `aida plan verify` green on this plan.

## 8. Verification (executable)

```bash
aida plan verify docs/plans/2026-06-12-cli-reference-manual.md
# drift-guard, once built:
aida help-all | <extract commands> | <assert each has a docs/cli entry>   # exit non-zero on omission
# spot-check a chapter grounds correctly: every backticked `--flag` in a chapter exists in that cmd's --help
```

## 9. Followups

- Consider an `aida manual <command>` that prints the rationale entry next to `--help` (rationale-strings-beside-clap), making the manual *generated* — the drift-proof endgame. File only if the hand-authored manual proves its value first (don't build the framework before the content).
- Man-page output (`clap_mangen`) for the *facts* half — complements, doesn't replace, the rationale manual.

## 10. Related

- **Specs:** EPIC-40 + chapter children + drift-guard slice.
- **Docs:** `docs/lifecycle.md` (state machine — linked, not duplicated), `docs/lifecycle.html` (rendered), `docs/aida/discipline/` (vocabulary), `README.md` "CLI reference (authoritative)" section (the terse current pointer this supersedes in depth).
- **Memories:** precise-lifecycle-vocabulary, failed-flag-attempts-are-ux-signals (chapters will surface these), run-help-before-suggesting-flags (D3).
