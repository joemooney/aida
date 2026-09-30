# BUG-1742 — what actually reds the required Linux build on `main`

Date: 2026-09-30 · Spec: BUG-1742 · Measurement: `scripts/ci-flake-tally.py`

## The headline, up front

BUG-1742 was filed during a burst and describes the burst, not the baseline.
Its premise — "roughly hourly, a different `aida-cli-lib` test each time" — is
half right. The rate was real and the burst was real, but:

- The failures are **not** a different test each time. Nine distinct tests
  produced fourteen failure events; four of the nine repeat.
- They are **not** all in `aida-cli-lib`. Two are in `aida-core` (`db::*`).
- They do **not** share one mechanism. There are **four** families.
- **Eight of the nine tests already had a filed spec**, and for seven of them
  the fix had already landed or was landing the same day. Exactly **one**
  offender was unfiled and still live.

The right shape of work was therefore never "five separate fixes". It was one
measurement, one attribution pass, and one new spec.

## AC1 — the rate, with the denominator pinned

Window: every completed `CI` run on `main` from `push` since **2026-09-20**
(302 runs — more than the 50-run floor the acceptance allows, so the date
window governs), through 2026-09-30T14:42:56Z.

| quantity | value |
|---|---|
| completed `main`/`push` CI runs in window | 302 |
| excluded: docs-only short-circuit (`Run tests` skipped) | 15 |
| **denominator: runs that actually executed the suite** | **287** |
| runs whose `Build (ubuntu-latest)` job failed | 12 |
| **failure rate** | **12/287 = 4.2%** |

Every one of the 12 failures was the **`Run tests`** step. No failure in the
window came from formatting, clippy, a guard, or infrastructure — which is
worth stating, because it means the required check's redness is entirely a
test-flake story and not a mixed one.

**The only exclusion is the 15 docs-only runs.** A docs-only push skips the
`Run tests` step via `Detect full-CI changes`, so it cannot flake; counting it
as a clean pass would have reported 4.0% instead of 4.2%. Nothing else was
dropped: no run was cancelled, timed out, or re-run in the window, and every
failing run failed on attempt 1.

### Per-test tally

Reported for every test, including the ones that failed once, per the
acceptance.

| n | test | family |
|---|---|---|
| 3 | `scaffold_refresh::tests::starter_role_refresh_adds_missing_prompt_and_preserves_user_prompt` | D |
| 2 | `bug_1452_refusal_aftermath_tests::refusal_aftermath_is_parked_held_and_awaiting_visible` | **B — live** |
| 2 | `bug_1291_orphan_sweep_tests::queue_add_failure_is_propagated` | A |
| 2 | `maintenance_schedule::tests::tick_lock_is_nonblocking_and_reusable` | C |
| 1 | `bug_1609_gitlab_reviewer_preflight_tests::pr_base_head_resolves_real_gitlab_mr_branches_via_forge_not_gh` | A |
| 1 | `story1163_forge_dispatch_tests::diff_change_dispatches_gitlab_to_glab` | A |
| 1 | `bug_1670_stale_cache_callers_tests::unambiguous_write_resolution_sees_external_collision_under_foreign_lock` | C |
| 1 | `db::cache_refresh::tests::nested_freshen_in_lock_holder_does_not_wait` | C |
| 1 | `db::cached_git_backend::tests::migration_pending_reader_never_serves_old_schema` | C |

### The rate is not uniform, and that matters

| sub-window | rate |
|---|---|
| 2026-09-20 .. 2026-09-28 | 6/262 = **2.3%** |
| 2026-09-29 .. 2026-09-30 | 6/25 = **24%** |

BUG-1742 was filed inside the right-hand column. A reader who takes "roughly
hourly" as the steady state will over-invest; a reader who takes 2.3% as the
steady state will under-invest. Both numbers are in the table on purpose.

## AC3 — one mechanism, or independent? **Independent: four families.**

The acceptance asks this explicitly because the shared-mechanism answer would
make a per-test fix list the wrong shape of work. It does not. The evidence is
the panic text, which differs in kind between families, plus the fact that each
family already has its own spec lineage.

**Family A — exec of a just-written injected binary (4 events, 3 tests).**
`story1163` names it outright: `could not invoke 'glab' … Caused by: Text file
busy (os error 26)`. `bug_1291` and `bug_1609` are the same shape (a fixture
binary the process just wrote, spawned from a sibling thread's fork window),
though their logs print no `Caused by`, so the errno was not captured — see
the gap noted below. Lineage: BUG-468 → BUG-1202 → BUG-1544 → BUG-1689 →
**BUG-1735**, which on 2026-09-30T10:51Z swept 38 files routing injected-binary
spawn sites through `output_retrying_etxtbsy()`. The `bug_1291` call site in
`aida-cli-lib/src/lib.rs` now carries `trace:BUG-1735` — the fix landed on
exactly the line that failed. **All four events predate that merge.**

**Family B — a file under a `tempfile` temp root vanishing mid-test
(2 events, 1 test).** `bug_1452_refusal_aftermath` failed twice with two faces
of one symptom: on 2026-09-26 the write failed (`could not write
/tmp/.tmpclrg5o/.aida/review-verdicts/PR-1452.json: No such file or directory`),
and on 2026-09-30 the write succeeded and the read-back found nothing (`the
phase-3 handshake … disappeared immediately after being written`). The second
message only exists because BUG-1571 added a belt-and-suspenders `is_file()`
check after the write — without it this would have been a silent success.
**This is the only family with no spec of its own, and the only one whose most
recent failure postdates every fix in the window.** Filed as BUG-1743.

**Family C — flock / refresh-lock contention (5 events, 4 tests).** Panics
about lock acquisition: `re-acquire after the last guard dropped returned None;
… direct_flock=Some((WouldBlock, …))`, `the refresh flock must be free before
the holder takes it`, `assertion failed: try_tick_lock(…).is_some()`. Lineage:
BUG-1303 → BUG-1595 (`tick_lock`, completed 2026-09-24) and BUG-1729 →
**BUG-1736** (completed 2026-09-30T11:42Z) for the `aida-core` refresh-lock
helpers, with the investigation record in
`docs/plans/2026-09-29-refresh-lock-load-sensitivity.md`. **All five events
predate their respective fixes.**

**Family D — a genuine regression, not a flake (3 events, 1 test).**
`starter_role_refresh…` failed on three *consecutive* runs on 2026-09-21/22
(`legacy role should be refreshed`) and never again in the eight days since.
A flake does not cluster three-for-three and then stop; a bug that gets fixed
does. It was: **BUG-1579**, completed 2026-09-22. Counting this as flake noise
would have inflated the flake rate by 25%.

The families do share a *precondition* — CI parallelism and load — but a shared
precondition is not a shared mechanism, and the remedies are unrelated (a spawn
retry, a temp-root ownership question, lock-acquisition backoff, a product
fix). Five separate fixes would indeed have been the wrong work; so would one
unified fix.

## AC2 — disposition per recurring offender

No test was quarantined, retried, or `#[ignore]`d. Nothing in this pass touches
test selection.

| test | disposition |
|---|---|
| `starter_role_refresh…` | Fixed at cause — **BUG-1579** (completed 2026-09-22). 8 days clean. |
| `tick_lock_is_nonblocking_and_reusable` | Fixed at cause — **BUG-1595** (completed 2026-09-24). 6 days clean. |
| `bug_1291…`, `bug_1609…`, `story1163…` | Fixed at cause — **BUG-1735** (merged 2026-09-30T10:51Z). All events predate it. |
| `nested_freshen…`, `migration_pending_reader…`, `bug_1670 stale_cache…` | Fixed at cause — **BUG-1729 / BUG-1736** (merged 2026-09-30T11:42Z), record in `docs/plans/2026-09-29-refresh-lock-load-sensitivity.md`. All events predate it. |
| `refusal_aftermath_is_parked_held_and_awaiting_visible` | **Unfiled and live. → BUG-1743**, with the two panic texts and both run IDs attached. |

### What this pass deliberately does not claim

Only **4 runs** in the window postdate both BUG-1735 and BUG-1736, and one of
them failed. **Four runs is not evidence that anything was fixed**, and the
25% that arithmetic yields is noise, not a rate. The attribution above rests on
the per-family timeline (every event of families A, C and D predates its own
fix) and, for family A, on the fix having modified the exact failing call site
— not on a post-fix green streak, which does not yet exist. Whoever re-runs
`scripts/ci-flake-tally.py` in a week will have the sample that settles it.

## AC4 — the measurement is repeatable

```bash
scripts/ci-flake-tally.py --since 2026-09-20      # reproduces every number above
scripts/ci-flake-tally.py --last 50 --json        # machine-readable
```

Two traps the script exists to avoid, both hit while producing this report:

1. **The GitHub server-side run filters returned stale data.** Both
   `gh run list --workflow ci.yml --branch main` and
   `/actions/workflows/<id>/runs?branch=main&event=push` reported their newest
   run as 2026-09-08 — three weeks stale — while the unfiltered listing of the
   same workflow was current to the minute. A tally built on either filter
   would have measured a window that ended before the burst it was filed about,
   found nothing, and closed the bug. The script paginates unfiltered and
   filters client-side.
2. **`gh run view --log-failed` returned the wrong step.** On these runs it
   labelled every line `UNKNOWN STEP` and returned the checkout teardown; the
   log contained no `test result:` line at all. The script reads
   `/actions/jobs/<job_id>/logs` directly.

## Follow-on worth having, not filed here

Family A's `bug_1291` and `bug_1609` panics printed no `Caused by`, so the
underlying errno was never recorded — the attribution to ETXTBSY rests on the
call-site shape and the fix's timing rather than on captured evidence. A
spawn-failure context that always names the OS error would have made this pass
a grep instead of an inference. Noted on BUG-1742 rather than filed, because
BUG-1735's sweep may already have changed these messages.
