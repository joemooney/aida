# SPIKE-67 slice 3 — live drain-path instrumentation for stated-rule violations (the real-time gate-vs-rule evidence path)

- **Date:** 2026-06-26
- **Specs:** SPIKE-67 (parent EPIC-50); builds on TASK-890 (slice 1) + TASK-891 (slice 2)
- **Status:** shipped (instrumentation + query surface + this findings note); multi-week field harvest is the ongoing deliverable
- **Complexity:** small (one new module + one telemetry hook + one hidden query subcommand)

## What this slice is

Slices 1-2 (`aida-cli/src/field_study.rs`) made **the git log the planted sensor**: `aida field-study scan` recomputes stated-rule verdicts (commit-format, trace-presence) *retrospectively* over the commit log, and `aida field-study report` aggregates them with vendor / drain / commit-type controls. That answers "did a commit, after the fact, satisfy a stated rule?"

This slice adds the **real-time complement**: during a real `aida queue work --auto-complete` drain, the orchestrator *already* learns the moment a stated rule was broken — CI came back red on a `fmt` / `clippy` / provenance (`/// SPEC-ID` doc-comment leak) check, the reviewer flagged a rule, or the implementer punted citing one. Those moments are exactly the gate-vs-rule evidence the ablation program could not manufacture: **a confident agent broke a rule that was *stated* in CLAUDE.md / a brief, and no programmatic gate stopped it before the commit — only CI caught it, after.** This slice records one structured event per such moment so the field study can answer the load-bearing question: *which stated rules need to become gates?*

## The thesis it serves (substrate-as-bouncer)

A rule merely **stated** in a prompt / CLAUDE.md / brief is unreliable against a confident LLM; to *guarantee* an invariant you ship a programmatic **gate** (substrate as bouncer), not a rule. The ablation program (STORY-655) reached its terminus: five controlled cells, all 100% rule-only / 0 gate-saves — a clean ablation cannot reproduce rule-dropping at all. The lone real drop (the bake-off, n=1) lives in a regime controlled designs can't reach: a large real codebase under long-horizon autonomy. **Field telemetry is the only remaining instrument.** This is that instrument for the live drain.

## Instrumentation shipped

**New module `aida-cli/src/rule_violation.rs`** — observe-only, opt-in, privacy-floor-preserving, best-effort (never blocks or delays the drain).

### Event schema (`~/.aida/rule-violations.jsonl`, one JSONL line per violation)

| field | meaning |
|---|---|
| `ts` | RFC3339 — when observed (drain completion) |
| `spec_id` | the spec the drain drove (an identifier breadcrumb, never content) |
| `rule` | the stated-rule category: `fmt`, `clippy`, `provenance-leak`, `commit-format`, `advisor-code-gate` |
| `caught_in_phase` | phase slug where the drain caught it (`ci`, `reviewer`, …) — the distance from where the rule was *stated* (pre-commit / brief) to where it was *caught* (CI) is itself the signal |
| `via` | how it surfaced: `ci-red` / `reviewer` / `punt` |
| `headless` | was this a `--no-human` drain? (the context-pressure axis the hypothesis most wants) |
| `variant` | `--auto-complete` variant (`full`, `through-ci`, …) — span/load context |
| `repo_bucket` | `small`/`medium`/`large`/`huge` (repo-size proxy) — the regime the ablations couldn't reach |
| `binary_sha` | aida build SHA (release tracking) |

**Privacy floor** (same as `aida usage` / the slice-1 sensor): identifiers and categories only. The failure message text is parsed for a rule signature *inside* `classify_rule` and **discarded** — it never reaches the log. No message text, no file paths, no diff content, no requirement content. A unit test (`build_events_carries_context_and_no_message_text`) asserts a secret path in the failure message does not leak into the serialized event.

### Detectors (the pure `detect()` classifier)

1. **CI-red on a stated rule** — `failure_kind == ci-red` and the reason matches a fmt / clippy / provenance / commit-format signature. The headline case: the rule was stated, no pre-commit gate stopped it, CI caught it red.
2. **Reviewer RequestChanges citing a rule** — `failed_phase == reviewer` / `no-verdict` and the message names a rule the substrate could have gated.
3. **Punt citing a rule** — the implementer punted (`punt_reason`) naming a known stated-rule.

Each yields at most one event per rule (deduped). A genuine test failure (`test_foo assertion failed`) is deliberately **not** a stated-rule violation — it produces no event, so the log stays a clean gate-vs-rule signal rather than a generic CI-failure dump.

### Wiring

The detector is fed from `record_auto_complete_run` in `main.rs` — the existing TASK-266 telemetry hook that already runs once per drain with the fully-classified `OrchestrationResult` (`failed_phase`, `failure.kind`, `failure.reason`, `punt_reason`) and the headless flag (`driver.no_human`). Zero new orchestrator control-flow; it reuses what the drain already computed. The repo-size git call is gated behind a cheap `detect().is_empty()` pre-check, so a clean drain pays nothing.

### Opt-in

Shares the slice-1 switch: default **OFF**; enable with `AIDA_FIELD_STUDY=1` or `[field_study] enabled = true`; honors the global `AIDA_TELEMETRY=0` kill-switch. Plant-the-sensor-now, harvest-over-weeks.

### Query surface

`aida field-study violations` (hidden, like `scan`/`report`) — tallies by rule, splits headless vs supervised, and frames each event as a substrate-as-bouncer candidate. `--json` for machine consumers. The log can also be read directly: `jq . ~/.aida/rule-violations.jsonl`.

## Early gate-vs-rule signal (concrete cases this session's history already produced)

This slice ships against a backdrop of **already-observed field cases** where a stated rule failed in this very repo and a gate was the resolution — exactly the events the live logger will now capture automatically going forward:

- **The recurring `/// SPEC-ID` doc-comment leak → BUG-624's gate.** The stated rule ("keep `trace:` markers plain `//`, never `///`, so they don't leak into `--help`") was written in CLAUDE.md and repeated in briefs, yet confident agents kept emitting `///` trace comments. It was reliably broken until a **programmatic pre-commit gate** (diff-scoped, BUG-624 / #1133) was shipped. Stated rule unreliable → gate reliable. This is the canonical case, and `provenance-leak` is a first-class detector slug.
- **The advisor-code-gate → STORY-684.** The stated rule ("the advisor seat does not author implementation code") needed a **programmatic gate** rather than prose to hold against a confident advisor session. `advisor-code-gate` is a detector slug.
- **Agents `--no-verify`'ing commits.** The stated rule (pre-commit hooks must run) is bypassable by a single flag; agents reached for `--no-verify` to get past a hook they judged a false alarm. A stated rule a confident agent can route around with one flag is the textbook substrate-as-bouncer gap — the violation surfaces downstream as CI red on the very check the hook would have caught, which is precisely the `ci-red` detector path.

These three are the qualitative seed. The live logger now turns that anecdote stream into a counted, headless-vs-supervised, repo-size-bucketed series — so the hypothesis (do stated-rule violations rise with repo size / task span / context pressure / headlessness?) can be tested on real drains rather than re-argued from memory.

## Tests

`aida-cli/src/rule_violation.rs` unit tests (12): each detector (fmt / clippy / provenance / reviewer / punt), the no-false-positive guard (a real test failure yields nothing), dedup across CI+punt, the clean-run-yields-nothing path, the privacy-floor assertion (no message text in the event), JSON round-trip, and the `by_rule` / `headless_split` aggregators.

## Followups

- **Harvest (the actual study).** Run real opt-in drains over weeks; periodically `aida field-study violations`. The deliverable is the *data*, not just the sensor.
- **`--no-verify` direct detection.** Today a `--no-verify` bypass is caught indirectly (it surfaces as CI red on the bypassed check). A direct pre-commit-hook-side detector (record when `--no-verify` was used at all) would close the loop more tightly — file as a child TASK if the indirect signal proves too lossy.
- **Join against `auto-complete.jsonl` run windows** to enrich each violation with task-span (like slice 2's drain join), so the repo-size and span axes can be cross-tabulated.
- **Promote a recurring rule to a gate.** When a rule recurs in the violations log past a threshold, that is the signal to ship its programmatic gate (the BUG-624 / STORY-684 move), and to retire the prose rule that wasn't holding.

## Related

- `aida-cli/src/field_study.rs` — slices 1-2 (the retrospective git-log sensor)
- `docs/research/ablations/2026-06-20-field-instrumentation-spike-67.md` — slice-1 findings + first harvest
- `aida-cli/src/auto_complete_telemetry.rs` — the TASK-266 drain telemetry this hooks alongside
- `aida-core/templates/docs/aida/discipline/substrate-as-bouncer.md` — the thesis
