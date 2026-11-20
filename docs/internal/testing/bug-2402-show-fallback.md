# BUG-2402: retained recurrence and rejected snapshot locators

<!-- trace:BUG-2402 | ai:codex -->

## Retained evidence (2026-11-11)

Read-only analysis of `~/.aida/usage.jsonl`, selecting `cmd == "show"` and
`2026-11-07T15:40:14.840529418Z <= ts <= 2026-11-08T15:40:14.840529418Z`,
reproduces the committed `schedule/performance-guard.yaml` trip exactly:
485 calls, 67 above 2000 ms (13.814%), maximum 7578 ms. The configured
2000 ms budget, 10% tolerance and independent 5000 ms ceiling are unchanged.

The retained event schema records argument **counts**, not argument values
or individual flags. It has no cache-state or fallback-scan field. Consequently
these events cannot establish which native/agreed/padded/alias/UUID IDs,
`--comments`, `--full`, `--no-git`, PR lookups, or cache states produced the
slow samples. Argument count 3 cannot distinguish one boolean flag from any
other boolean flag. No such attribution is claimed.

| Arguments after executable | Calls | Above 2000 ms | Maximum ms |
| --- | ---: | ---: | ---: |
| 2 | 280 | 31 | 6410 |
| 3 | 147 | 31 | 7578 |
| 4 | 51 | 5 | 2664 |
| 5 | 6 | 0 | 1987 |
| 6 | 1 | 0 | 1587 |

For the 67 slow calls, retained phase evidence is:

| Phase | Median ms | Maximum ms | Calls above 1000 ms |
| --- | ---: | ---: | ---: |
| open.cache | 1240 | 4297 | 41 |
| notice | 121 | 1850 | 6 |
| lookup | 31 | 1136 | 1 |
| context | 134 | 2316 | 2 |
| linkage | 740 | 1844 | 27 |
| residual | 10 | 1832 | 16 |

Every slow call has these phases. The largest top-level phase was cache open
in 36 calls, linkage in 14, residual in 11, notice in 4 and context in 2.
Dotted linkage phases and `open` are nested, so they are not added again.
The 7578 ms call (2026-11-07T16:06:10.254891400Z, binary `122fea66d30`)
spent 4297 ms in cache open, 1501 ms in notice, 73 ms in lookup and 1230 ms
in linkage. This is evidence against treating the remaining lookup issue as
an explanation of the entire retained incident.

Current source also includes BUG-2466 changes: `9ac1e87d7c6` avoids allocation
and refresh-hint lock waits; `c1aca0d93c0` (2026-11-08 11:50:59 UTC) opens
show's projection without refresh and omits default linkage. A merge timestamp
alone does not establish deployment to the binaries in this window.

## Remaining source path and bounded fix

`CachedGitBackend::resolve_unambiguous_exact` first validates a derived locator
against canonical YAML (c0110e5). A rejected locator still fell through to
`get_requirement_by_spec_id` and then
`GitBackend::get_requirement_by_exact_spec_id`. An agreed ID without its own
object file entered `agreed_id_store_scan`, which enumerated and parsed objects
without show's shared snapshot deadline. This occurs with a changed UUID,
moved/removed agreed owner, or stale collision candidates. It is a demonstrated
source path, not a scan classification recovered from historical telemetry.

Snapshot-only non-strict reads now use the existing `resolve_by_bounded_scan`
on that branch, with the existing invocation deadline and incomplete-evidence
warning. Strict writes and ordinary non-snapshot backends retain their previous
resolver. Literal/padded files are still read first; canonical UUID and ID checks
still guard the fast locator path; new collisions in fallback still refuse.

The literal-first resolver also remembers which exact input IDs already ran
a fallback, so a negative result cannot cause the outer stale-snapshot check
to repeat the same scan. Literal and expanded alias IDs remain distinct checks.
Neither cache rows nor bounded incomplete scans acquire write authority.

## Mechanism regression

`db::cached_git_backend::tests::bug2402_locator::show_snapshot_rejected_locator_bounds_store_scans`
uses the existing temporary-store/explicit HOME-authority fixture. After building
a projection it replaces the original object's UUID/agreed ID and moves the
agreed owner to another canonical path. Native, padded and alias agreed-ID
queries each read current canonical content using exactly one bounded-ID scan
and zero legacy agreed-ID scans. A new collision refuses with the same work
bound; unrelated malformed YAML is tolerated. An already-spent deadline returns
incomplete/not-found after one scan rather than entering the unbounded legacy
scan, while a literal canonical hit remains readable. Cache bytes stay unchanged.
Assertions use scan counters and a pre-spent deadline, not elapsed-time limits.

This fixture establishes the remaining fallback mechanism and its work bound.
It does not establish fleet SLO recovery or a deployed before/after timing
comparison; those need retained telemetry with adequate attribution and a known
deployed artifact. The existing `closure:pending` tag is not removed here.

## Lane validation

The final targeted lane job `j20261111T142454845-2-0` executed the regression:
1 passed, 0 failed (2325 filtered out). `rustfmt --check` on the changed Rust
file and `git diff --check` passed.

The subsequent launcher job `j20261111T142611571-327110-0` ran
`make check-ci-fast`. It passed the earlier gates, including formatting,
containment, documentation, shell protocols, environment/import ratchets and
routing security fixtures, but timed out after the fast job's fixed 600-second
limit during `Check aida-cli without default features`. Exit 124 is **not** a
passing aggregate gate. The host must finish that validation before publishing
as completed work. No repeated capped compile or slot-limiter bypass was used.
The publication handoff therefore retains a Refs trailer.
