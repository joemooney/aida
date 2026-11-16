# Shift cleanup and ledger discovery

<!-- trace:BUG-2626 | ai:codex -->

The tick calls `RealExec::reap`, then `session_reap::reap_quiet_outcomes`.
Previously that path called the interactive `scan_reapable` report builder.
In addition to lease eligibility it collected unshipped work across historical
branches. `collect_unshipped_work_items_bounded` reads each surviving branch's
spec verdict; the single-key verdict reader can discover the entire gate ledger
for ownership filtering. Quiet cleanup discarded the resulting `unshipped_work`
field. This is an unnecessary source of repeated payload discovery, independent
of the current set of leases.

Quiet cleanup now scans lease eligibility and orphan pins directly. Interactive
reap reports still add unshipped diagnostics. All lease eligibility probes and
mutation checks remain shared. The `bug_2626_quiet_reap_skips_unshipped_discovery`
regression exercises repeated quiet cleanup with 100 unrelated ledger files and
checks that the diagnostic collector is never called; the interactive control
checks that diagnostics still run without leases.

This is a partial fix, not full acceptance evidence. `shift::held_specs` still
calls `hold::candidate_prs`, which reads every active PR ledger to search for hold
events, and then folds the matching ledgers. Thus an unchanged tick still has
work proportional to active ledger history. A by-spec-only shortcut cannot
safely replace this: indexes are best-effort hints, and successful event appends
can precede failed index writes. The approved completeness/bootstrap prerequisite
is described in `docs/plans/2026-11-12-bug-2531-discovery-retention-sketch.md`;
it is absent from this checkout. Hold discovery also needs complete hold-family
bindings (including old unindexed holds) before relevant-only reads are safe.
No missing index may turn a live hold into an absence.

Required follow-up evidence is a whole-tick work guard covering hold discovery,
plus a cron measurement against the current shared ledger after deployment.
A fast lock-contended tick is not that measurement. This seat does not run a live
tick that could dispatch work or claim acceptance from static source inspection.

Validation in the native lane (2026-11-16):

- Changed Rust files pass `rustfmt --edition 2021 --check`; `git diff --check`
  passes.
- Launcher job `j20261116T082156268-2-0`, filter `bug_2626`: 1 passed,
  0 failed.
- Launcher job `j20261116T082742941-2-0`, filter `session_reap::`: 98 passed,
  1 failed. `dead_reviewer_of_a_merged_pr_is_reaped_and_its_hold_adopted`
  reaches reap eligibility but deletion refuses because the fresh mail-retention
  writer-admission scan cannot verify every mail writer. The worktree is kept.
- Exact isolated rerun `j20261116T083049053-2-0` repeats that refusal (0 passed,
  1 failed). No retention admission or deletion checks were weakened.
- No uncontended production cron measurement was performed. The full spec is
  Needs Attention, blocked by BUG-2531, with no completion declaration.
