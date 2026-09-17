# Plan: STORY-1178 — Reconstitution loop v1

Date: 2026-09-16
Specs: STORY-1178 (child of EPIC-70; references EPIC-66)
Status: Draft
Complexity: ~1400 prod LOC, ~700 test LOC, 4 slices / ~6 commits, risk medium

<!--
  trace:STORY-1178 | ai:claude
  Symbol refs over line refs throughout.
-->

## Approach

The aida-store is today a lossy ONE-WAY summary of the code (spec → code): implementation
makes semantic decisions (ordering rules, error codes, validation limits, soft-vs-hard delete,
empty-state behavior) that never flow back. v1 makes the flow two-way and MEASURABLE, framed
as a compiler round-trip: the store is the source, tests are the executable half of the spec,
and a reconstitution probe is the round-trip test (`compile(source) ≈ original`).

Decomposed as **vertical slices** (each demo-able end-to-end on a real AIDA spec), not the
A/B/C layers the story lists:

1. **Slice 1 — tracer bullet:** criteria IDs + `trace:SPEC.ACn` on tests + `aida criteria <spec>`
   gap report. Proves the whole pipeline (parse criteria → scan tests → report) and immediately
   answers "how much of AIDA's own truth is in the store" on STORY-1171's merge_lock tests.
2. **Slice 2 — harvest, standalone:** `aida harvest <spec> [--diff <ref>]` — an agent reads the
   diff + criteria/ADRs, emits filtered candidates, the human confirms a checklist, confirmed
   items land in the store, the run is ledgered. Standalone first so value lands without the
   EPIC-66 pipeline.
3. **Slice 3 — reconstitution probe:** `aida reconstitute <spec>` — an agent regenerates the
   slice's tests from store content only, diffs against the real traced tests, emits a
   divergence report + score, and feeds non-reproducible tests back as harvest candidates.
4. **Slice 4 — pipeline integration:** wire the harvest gate into the EPIC-66 drain gate as an
   advisory PR/merge-time phase. Depends on the STORY-1156/1157 gate plumbing (see Risks).

### Diagram (optional but high-value)

```
  spec (criteria AC1..ACn, ADRs, SEM decisions)              ┐  the SOURCE
        │                                                    │
        │ trace:SPEC.ACn                                     │
        ▼                                                    │
  real tests  ──── aida criteria ────►  gap report           │  slice 1
        │            (untested ACs / unanchored tests)       │
        │                                                    │
   PR diff ──── aida harvest ──►  candidates ──confirm──► store (AC / [aida:sem] / ADR stub)
        │      (observable ∧ non-obvious filter)   └──────► ledger [aida:harvest]      slice 2
        │
  store only ── aida reconstitute ──► regenerated tests ──diff vs real──► divergence + score
                                                              └──► harvest candidates  slice 3
  EPIC-66 drain gate ── advisory harvest phase at PR/merge time                         slice 4
```

## Decisions

- **Criterion IDs = `<SPEC>.<label>`; explicit labels win, content-hash for unlabeled.** A line in
  the `## Acceptance` section beginning with a label (`A1.`, `AC3.`, `B2:`) gets that label;
  an unlabeled bullet gets `ac<6-hex>` of its normalized text. Stable across reorders; an
  *edited* unlabeled criterion re-IDs (documented; nudge: label your criteria). No schema change.
- **SEM micro-decisions are marked comments, not a new type (v1).** `[aida:sem]` structured
  comments on the spec — the exact STORY-1173 `[aida:proxy-approval]` ledger pattern: zero schema
  churn, git-canonical, aggregatable by marker. A first-class `SEM-N` type is a followup if
  volume warrants.
- **Harvest + probe agents are headless `claude -p` runs via the existing reviewer/advisor-tier
  launcher** (`spawn_claude_headless` / `compose_headless_command`), returning structured JSON.
  No new agent transport.
- **Selectivity filter lives in the prompt AND a `[harvest]` config section** with a conservative
  default (`min_confidence`, `skip_conventional = true`, an allow/deny keyword list). The filter
  is the calibration surface; start strict (anti-slop), loosen on evidence.
- **v1 harvest is ADVISORY, never blocking.** Every run writes an `[aida:harvest]` ledger comment
  (ran / confirmed / skipped) so skipped candidates are auditable, never silently dropped.
- **Probe scoring = fraction of real traced tests with an agent-judged matching regenerated
  test, per criterion.** The *divergence report* is the real product; the score is a trend line.
  Agent-judged matching is a heuristic — say so in the output.
- **Probe input is store-only by construction:** the probe prompt is assembled from
  `aida show --full` + linked ADRs/`[aida:sem]` comments + `aida graph` context, and the agent runs
  in a scratch worktree with the crate's test dirs removed, so it cannot peek at real tests.

## Files (in build-order)

### `aida-cli-lib/src/criteria.rs` — shipped by TASK-1246 (slice 1)
Parse a spec's `## Acceptance` section into `Vec<Criterion { id, label, text }>`; scan test
sources for `trace:SPEC.ACn`; compute the two gap classes; render human + `--json`.

### `aida-cli-lib/src/lib.rs` — `parse_trace_id_token` (slice 1)
Extend the trace-token parser to accept the `.ACn` suffix (`STORY-1178.A1`) alongside bare spec
ids, so `aida why` and the trace graph keep working and criteria tracing rides the same parser.

### `aida-cli-lib/src/cli.rs` — `Command::Criteria`, `Command::Harvest`, `Command::Reconstitute`
Three new read-mostly commands (slices 1–3). Place each variant AFTER an existing variant's
closing brace, never between a variant and its doc-comment (the exact #1873 regression).

### `aida-cli-lib/src/harvest.rs` — NEW (slice 2)
Build the harvest prompt (diff + criteria + ADRs), launch the headless agent, parse candidates,
apply the selectivity filter, drive the confirm-checklist (`AskUserQuestion`-style at a TTY /
`--yes-all` headless), write confirmed items, append the `[aida:harvest]` ledger comment.

### `aida-cli-lib/src/reconstitute.rs` — NEW (slice 3)
Assemble store-only context, run the probe agent in a scratch worktree, diff regenerated vs real
traced tests, score, emit the divergence report, emit harvest candidates.

### `aida-cli-lib/src/auto_complete.rs` — harvest phase (slice 4)
An advisory `Phase::Harvest` between review and merge in the EPIC-66 gate pipeline, gated by
`[harvest] gate = "advisory" | "off"`.

### `aida-cli-lib/src/presence.rs` — the `[presence]` config reader is the pattern to copy for `[harvest]` (slice 2)
`harvest.rs` reads its `[harvest]` section from `.aida/config.toml` the same way `presence.rs`
reads `[presence]` (config sections are parsed in `aida-cli-lib`, not a core template); document
the section alongside the other `[...]` sections in the config reference.

### `docs/cli/*.md` + `docs/environment-variables.md` — manual entries for the three commands
(the CLI-manual drift-guard's completeness check will fail the PR without them).

## Critical Files

- `aida-cli-lib/src/lib.rs` — `parse_trace_id_token`: shared by `aida why`, the trace graph,
  and `doctor_validate_trace_comments`. The `.ACn` extension must be backward-compatible (a bare
  spec id still parses identically) or every trace consumer regresses.
- `aida-cli-lib/src/cli.rs` — `Command` enum: variant placement bit us on #1873; the
  `visible_catalog_requires_about_for_every_command` test is the guard.
- `aida-cli-lib/src/session.rs` — `spawn_claude_headless` / `compose_headless_command`: the
  agent transport for both harvest and probe; do not fork a second launcher.
- `aida-cli-lib/src/criteria.rs` — the `.ACn` parser + gap classifier every later slice
  reads; shipped by TASK-1246, so slices 2-4 extend it rather than re-parse.
- `aida-cli-lib/src/harvest.rs` — the harvest gate is the drain-facing surface; its
  refusal semantics must match `auto_complete.rs` shelve causes.
- `aida-cli-lib/src/reconstitute.rs` — the scratch-worktree probe must never see real
  tests; the isolation is the whole measurement.
- `aida-cli-lib/src/auto_complete.rs` — new harvest phase slots into the closed
  phase/cause sets; extend the closed-set tests, do not bypass them.
- `aida-cli-lib/src/presence.rs` — the `[presence]` config reader is the pattern the
  `[harvest]` reader copies; keep the two parsers shaped alike.
- `docs/environment-variables.md` — every new `AIDA_*` read must land a row here in the
  same change (repo rule).

## Reusable helpers (do not reimplement)

- `fn parse_trace_id_token(line) -> Option<String>` (lib.rs) and
  `fn extract_trace_line_spec_ids(message)` — the trace parsers to EXTEND, not duplicate.
- `fn doctor_validate_trace_comments(...)` (doctor_cmd.rs) — already walks every source file for
  `trace:` comments; reuse its file walk + filter for the test scan in `criteria.rs`.
- `spawn_claude_headless`, `compose_headless_command`, `spawn_vendor_headless_with_seat`
  (session.rs) — headless agent launch with the vendor/seat plumbing; harvest + probe use these.
- The `[aida:proxy-approval]` marked-comment ledger pattern (STORY-1173) — copy its shape for
  `[aida:sem]` (micro-decisions) and `[aida:harvest]` (run audit).
- `docs/cli/verify-manual.py` reflection check (`interface_changes` reflected in the manual) — a
  narrow harvest gate already; its "spec field ↔ artifact" diff is the pattern for slice 2.
- `aida lint` (EARS testability lens) — criterion-quality checks; call it from
  `aida criteria --lint` rather than re-implementing "is this criterion testable".
- `aida ultraplan`'s `## Acceptance` section splitter — the same section parse `criteria.rs` needs;
  extract it to a shared fn rather than a second parser.
- Prune from the trace-graph seed: STORY-686 / TASK-1003 / STORY-1091 / TASK-1036 / ADR-28 /
  ADR-10 / TASK-1050 matched on shared tags only and are NOT relevant here.

## Risks + gotchas

- **Slice 4 depends on EPIC-66's gate plumbing (STORY-1156/1157), which is keystone and unbuilt.**
  Mitigation: slices 1–3 are standalone-invocable and deliver full value without the pipeline;
  slice 4 is last and may wait on EPIC-66.
- **Harvest selectivity = slop risk.** A loose filter floods the store with trivia and erodes
  trust; a strict one misses semantics. Start strict; the probe (slice 3) is the honest check
  that the harvest is capturing what matters. Track precision on the first 20 harvests.
- **Probe can peek at real tests** unless isolated. Run it in a scratch worktree with test dirs
  stripped; assert in a test that the prompt contains no test source.
- **Agent-judged test matching is fuzzy** → the score is a heuristic trend, not a proof. Surface
  the divergence report as primary; label the score "heuristic".
- **Content-hash criterion IDs re-ID on edit**, silently orphaning `trace:SPEC.ac<hash>` on tests.
  Mitigation: `aida criteria` reports a trace pointing at an unknown ID as a gap (it already must
  for the harvest-gap class); nudge toward explicit labels in `aida lint`.
- **Cost**: each harvest/probe is an LLM run. Scope the probe to one spec's slice; keep harvest
  advisory and skippable (`--no-harvest`).
- **Trace-token parser is load-bearing** (`aida why`, doctor, trace graph) — extend with a
  regression test that bare ids still parse byte-identically.

## Tests (named, not "add tests")

- `criteria::tests::labeled_and_unlabeled_criteria_get_stable_ids` — labels win; hash for
  unlabeled; reorder does not re-ID.
- `criteria::tests::gap_report_flags_untested_ac_and_unanchored_test` — one AC without a test,
  one test tracing an unknown AC.
- `parse_trace_id_token_accepts_ac_suffix_and_keeps_bare_ids_identical` (lib.rs) — the
  backward-compat guard.
- `harvest::tests::filter_drops_conventional_keeps_observable_nonobvious` — the selectivity
  filter on a fixture candidate set.
- `harvest::tests::skipped_candidates_are_ledgered_never_dropped` — `[aida:harvest]` records
  skips.
- `harvest::tests::confirmed_candidate_writes_ac_or_sem_or_adr_stub` — the three elevation
  targets.
- `reconstitute::tests::probe_prompt_contains_no_test_source` — the isolation guard.
- `reconstitute::tests::score_is_matched_over_total_real_traced` — deterministic on a fixture
  match table.
- `reconstitute::tests::divergence_feeds_harvest_candidates` — C4.
- `visible_catalog_requires_about_for_every_command` (existing) — must stay green after the
  three new `Command` variants (the #1873 lesson).

## Verification

```bash
# slice 1: the diagnostic, on a real spec
aida criteria STORY-1171            # AC → tests, untested ACs, unanchored tests
aida criteria STORY-1171 --json | python3 -c 'import sys,json;d=json.load(sys.stdin);print(d["untested"],d["unanchored"])'

# slice 2: harvest a real merged PR, standalone
aida harvest STORY-1171 --diff origin/main~3   # candidates → confirm → store + [aida:harvest]
aida show STORY-1171 | grep -c 'aida:harvest'  # ledger present

# slice 3: the round-trip score
aida reconstitute STORY-1171 --json | python3 -c 'import sys,json;print(json.load(sys.stdin)["score"])'

# guards
env -u AIDA_SESSION_ROLE cargo test -p aida-cli-lib criteria harvest reconstitute parse_trace_id_token
env -u AIDA_SESSION_ROLE cargo test -p aida-cli-lib visible_catalog_requires_about_for_every_command
cargo fmt --all -- --check && cargo clippy -p aida-cli-lib -- -D clippy::correctness
PATH="$PWD/target/release:$PATH" python3 docs/cli/verify-manual.py   # completeness for the 3 new cmds
```

## Followups

- First-class `SEM-N` record type if `[aida:sem]` comment volume warrants (schema change).
- Schema/API-contract capture as store artifacts (EPIC-70 mechanism 5).
- Store-side reverse index spec → implementing files/tests (EPIC-70 mechanism 6).
- `reconstitutable` lifecycle flag earned by passing the probe.
- Multi-language test scanning (v1 scans Rust `#[test]` fns).
- Slice 4 blocking mode (`[harvest] gate = "block"`) once precision is proven.

## Related

- EPIC-70 (parent: AIDA as a compiler), EPIC-66 (gate pipeline host; STORY-1156/1157 plumbing).
- STORY-1173 (`[aida:proxy-approval]` ledger — the marked-comment pattern reused).
- STORY-597 (CLI-manual drift-guard; its reflection check is the narrow-harvest precedent).
- TASK-0417 (`aida lint`), STORY-785 (`aida why file:line`), BUG-1177 (the phantom `Ultraplan`).
- ADR-38 (merge-lease — the last guided plan; same sketch-first cadence).
