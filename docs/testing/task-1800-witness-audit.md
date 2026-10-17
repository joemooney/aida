# Frozen-50 mandate refusal witness audit

<!-- trace:TASK-1800 | ai:codex -->

The manual audit of the **27 additional trace-refusal witnesses** finds **6 true
supervised touches, 18 incidental matches, and 3 unresolved cases**. These are
judgments about the selected witnesses, **not 18 proven false-positive PRs**.
Another exclusion in the same PR may still justify refusal. This measurement
changes no collector, authority rule, or mandate decision.

The [incremental semantic audit](task-1800-semantic-audit.md) now inspects every
blocker in fifteen cases, including full-file-only reasons. It retains unresolved
evidence and leaves the frozen-50 classification gate open.

The frozen outcome remains 22 named refusals plus 27 additional trace refusals,
49/50 total, in both full-file and scoped modes. Among the 22 named witnesses,
20 correspond to explicitly implemented supervised work; two are unresolved
combined branch IDs. One case has no policy refusal. Neither an unresolved name
nor unreadable history is counted as a true supervised touch just because it
fails closed.

[The data](task-1800-witness-audit.json) covers all 50 pinned cases, with the
selected witness, manual rationale, exact base/head trace locations, changed
ranges, and every fallback path. This extends the
[collector remeasurement](task-1800-trace-exclusions.md); it does not replace its
pins or policy counts. The collector is the module-coverage version at
`43839973d60cdd23081f2203d876bc97dd5f36be`, SHA-256
`7041c86bc20415191b8814875bdfc77e9aa6e8e904aea8894b9e64724e099dd8`.
The frozen store remains `a5dae7145abd11d2480271c466030b12db6e7668`; the private
five-entry ledger's sorted compact JSON hash remains
`753d1a323b18af13ed73a18f20f9a6f7c6242e3584edf01049897d0dba7289af`.
The ledger contents are not included.

## What drives fallback

Each case/path pair is counted once even when both committed sides fall back.
Classes are mutually exclusive: templates/mirrors and workflows take precedence
over file extensions; docs means Markdown under `docs/`; other paths are
explicitly retained in the data. Case columns overlap between classes.

| Path class | Fallback pairs | Cases with fallback | Additional witness cases present in class |
| --- | ---: | ---: | ---: |
| Rust source | 174 | 44 | 24 |
| AGENTS/OVERVIEW | 31 | 16 | 2 |
| docs/*.md | 23 | 16 | 1 |
| Templates/mirrors | 20 | 6 | 0 |
| Workflows | 2 | 2 | 2 |
| Other | 40 | 18 | 1 |
| **Total fallback pairs** | **290** | **50 overall** | **27 overall, overlapping classes** |

The last column locates the selected additional-refusal witness in collected
scoped evidence, including paths that did **not** fall back. It is not the count
of all exclusions supplied by that class. A zero for templates/mirrors means
none of the selected witnesses came from that class; it does not establish that
templates contain no authoritative traces.

Rust supplies 60% of fallback pairs. Of the 290 fallback paths, **227 are
modified and 63 added**; among the 174 Rust paths, **122 are modified and 52
added** (no deletions or rename pairing). Added files have all their content in
the change, so counting them as fallback does not by itself identify avoidable
co-location. The JSON records each path’s change kind. Aggregate documents are
therefore only part of the problem. The 40 other pairs include 8 `scripts/config-trust.toml`,
2 `shared/types.ts`, 2 `.aida/discipline/git-sync-and-review.md`, 2 `Makefile`,
2 `scripts/ci-gate-tiers.toml`, and 24 singleton paths. These include other
source languages, local discipline files, manifests, generated data and test
scripts. They must not disappear into an unreported remainder.

## Manual judgments on the additional witnesses

A true touch requires a concrete change to the witnessed requirement's behavior
or its contract documentation. Incidental means the selected trace belongs to
an unchanged, different behavior or paragraph; merely sharing a file or long
function is insufficient. Unresolved means missing classification evidence or
an unproven relationship, and remains excluded. These judgments grant no
shipping or completion authority.

| PR | Witness | Judgment | Diff evidence |
| --- | --- | --- | --- |
| 2707 | BUG-1167 | True touch | Hold-marker writer is factored and re-arm audit context changes. |
| 2679 | TASK-1730 | True touch | Scorer invokes untrusted completeness helpers under confinement before the oracle. |
| 2701 | TASK-1337 | Incidental | Relay-stall documentation changes; the pickup/delegation paragraph is unchanged. |
| 2698 | BUG-1533 | Incidental | Lease cleanup moves; the system-mail sender envelope is unchanged. |
| 2673 | BUG-1924 | Incidental | Regeneration display is appended; grant-derived role display is unchanged. |
| 2695 | BUG-1167 | True touch | Attested approval can release a supervised hold. |
| 2684 | AUTH-0001 | Unresolved | Unchanged SQLite PIN-hash migration; no frozen spec resolves it. |
| 2697 | BUG-1167 | Incidental | Early default-branch refusal; later hold-clearance logic is unchanged. |
| 2678 | ADR-11 | Incidental | Regeneration consensus options change; `aida do` parser test is unchanged. |
| 2690 | ADR-55 | Incidental | Relay module is added; evaluator module declaration is unchanged. |
| 2685 | TASK-1335 | Unresolved | Evidence-query behavior is touched, but frozen historical classification is unavailable. |
| 2677 | ADR-11 | Incidental | Model-family scoring option; unchanged `aida do` parser test. |
| 2674 | ADR-54 | Incidental | Planner test step is added; GitLab perimeter-auth guard is unchanged. |
| 2671 | ADR-11 | Incidental | Handoff options/tests change; unchanged `aida do` parser test. |
| 2667 | ADR-11 | Incidental | Regeneration report verb; unchanged `aida do` parser test. |
| 2666 | TASK-1730 | True touch | Oracle environment capture and adapted second execution change. |
| 2675 | BUG-1533 | Incidental | Claim/reap handling changes; mailbox sender attribution is unchanged. |
| 2485 | ADR-10 | Incidental | Worker ownership changes; typed autonomy-mode contract is unchanged. |
| 2657 | ADR-10 | Incidental | Worker self-claim argument changes; autonomy-mode contract is unchanged. |
| 2669 | STORY-1226 | True touch, documentation | Scheduler full-tick/unsupported-job contract is documented explicitly. |
| 2658 | ADR-11 | Incidental | Regeneration corpus command; unchanged `aida do` parser test. |
| 2663 | ADR-10 | Incidental | Contained-Git preflight; autonomy-mode contract is unchanged. |
| 2656 | ADR-54 | Incidental | Sccache fixture step; unchanged GitLab perimeter-auth guard. |
| 2654 | STORY-1218 | True touch | Shift installation calls the changed shared tick invocation; its binary locator changes. |
| 2652 | BUG-1-051 | Unresolved | Unchanged deprecated pull wrapper; unresolved spec ID. |
| 2650 | BUG-1533 | Incidental | Recipient/mention filtering changes; sender attribution is unchanged. |
| 2647 | BUG-1978 | Incidental | Record-outcome display changes; identity-attribution fallback is unchanged. |

PR 2654 was initially uncertain because the changed scheduler file and the
night-shift spec use different command names. The pinned head resolves the
relationship: `shift.rs:3541` calls `schedule_driver::real_tick_invocation`, whose
changed implementation at `schedule_driver.rs:1335` now chooses the stable live
binary. This is a true installer touch and remains in the retention set.

The named unresolved witnesses are `BUG-2160-2161` in PR 2686 and
`BUG-2149-2150` in PR 2680. The frozen name extractor admits these combined
branch tokens. Resolving the component IDs is separate from claiming that the
combined ID names a supervised requirement. No parser change is proposed here.

## Evidence and limits

The author read each selected witness's committed trace context and pinned diff;
these are manual judgments, with their rationale in every row. For provenance,
a research-only copy of the exact collector emitted each `source_ids` result
before union. Its only instrumentation was an output statement: no selection
or fallback logic changed. All **50 scoped ID unions and all 50 fallback-path
sets** matched the module-coverage raw arrays exactly. This distinguishes a
trace present somewhere in a changed file from a trace actually selected by the
collector. Zero-based, half-open changed ranges in the data include empty-side
boundaries. Trace-line text retains the surrounding source evidence rather than replacing
it with the semantic label.

The report reuses the frozen refusal witnesses and classification inputs. It
does not claim a new end-to-end mandate replay, exhaustive current/history
classification of every collected ID, or independent review. The 20 named true
touches are a separate baseline stratum, not evidence of added trace precision.

**The measurement gate is still open.** Before reducing exclusions, enumerate
and adjudicate the other exclusion reasons in each candidate PR, resolve or
conservatively retain every unknown, and preserve every identified true touch.
An incidental selected witness alone cannot justify admitting a PR. In
particular, PR 2485 edits approval authority and PR 2663 edits containment even
though their selected ADR-10 witness is incidental. Their other traces need
explicit checking.

No path class is proposed as non-authoritative. Docs can change a supervised
contract, as PR 2669 demonstrates. This audit supports investigating precise
attribution for unrelated paragraphs, workflow steps, and Rust enum/module
edits; it does not authorize dropping document, template, or header evidence.
Symlinks, binaries and gitlinks retain the existing fail-closed behavior.
