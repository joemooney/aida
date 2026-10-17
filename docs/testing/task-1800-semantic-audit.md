# Frozen-50 semantic audit: ten complete blocker inventories

<!-- trace:TASK-1800 | ai:codex -->

Ten cases now have manual inspection of **every policy-blocking trace ID**, in
both full-file and scoped modes. This is an incremental measurement: **40 cases
remain**, and the classification gate is open. No refused case inspected so
far is proven incidental-only. PR2668 remains the frozen allowed case, with no
blocking trace IDs. No collector or policy behavior changes.

The [machine data](task-1800-semantic-audit.json) extends the
[selected-witness audit](task-1800-witness-audit.md). It inventories all 43
full-file case/ID pairs (37 scoped pairs), not just the first refusal witness:

| PR | Full/scoped blockers | True supervised touches | Incidental blockers | Unresolved blockers |
| --- | ---: | --- | --- | --- |
| 2647 | 6 / 4 | None established | BUG-1978, BUG-1983, STORY-1473†, STORY-1687 | EPIC-1-001, FR-0259† |
| 2701 | 4 / 4 | None established | TASK-1337, TASK-1491, TASK-1492 | TASK-176 |
| 2685 | 1 / 1 | Behavior touched; supervision unknown | None | TASK-1335 |
| 2654 | 4 / 4 | STORY-1218, STORY-1226, TASK-1491 | TASK-1492 | None |
| 2669 | 6 / 5 | STORY-1218†, STORY-1226, TASK-1491 | TASK-1337, TASK-1492 | TASK-176 |
| 2668 | 0 / 0 | None | None | None; frozen allowed case |
| 2681 | 2 / 2 | BUG-2148, TASK-1782 | None | None |
| 2686 | 6 / 4 | STORY-1218, TASK-1492, TASK-1730, TASK-1731† | STORY-1473†, TASK-1491 | No unresolved trace; named BUG-2160-2161 unresolved |
| 2653 | 4 / 4 | STORY-1130, TASK-1750 | BUG-1929, STORY-771 | None |
| 2698 | 10 / 9 | BUG-2139 | BUG-1533, BUG-631, FR-284, STORY-1218, TASK-1337, TASK-1491, TASK-1492, TASK-1561† | TASK-176 |

† Full-file mode only. Trace totals are **15 true supervised touches, 22
incidental matches, 6 unresolved** in full-file mode; scoped totals are
**13, 19, 5**. Counts are case/ID pairs, so a requirement present in two cases
counts twice. Four cases have no unresolved exclusion reason: PR2654, PR2681
and PR2653 have true supervised touches; PR2668 has no blockers and was already
allowed. The other six retain unresolved evidence, including the combined
named token in PR2686. The frozen conservative outcome remains **49/50
refusals**, comprising 22 named refusals and 27 additional trace refusals.

Four of the newly inspected cases also inventory **every extracted named token**:

| PR | Named tokens | Frozen named result and manual finding |
| --- | --- | --- |
| 2668 | TASK-1767 | Allowed; changed precedence memory directly implements the ordinary task |
| 2681 | TASK-1782 | Identity metadata refusal; directly touched |
| 2686 | BUG-2160, BUG-2160-2161, BUG-2161 | Component bugs directly touched and ordinary in the frozen all-ID census; combined branch token remains unresolved |
| 2653 | TASK-1750, TASK-1768 | Keystone TASK-1750 directly touched; TASK-1768 also directly touched and ordinary in the frozen all-ID census |

These named findings are separate from the trace-pair totals. The earlier five
cases and new PR2698 had no named refusal. No policy/history classification
was rerun.

PR2669's true parent touch, `STORY-1218`, is absent from the existing scoped ID
set even though other true blockers still refuse that PR. Case-level refusal
retention therefore does not prove retention of every true touched ID. Future
coverage checks must preserve this distinction; this measurement repairs no
collector gap. PR2686 adds another ID-level limitation: full-file-only
`TASK-1731` is a true touch through the changed survivor detector feeding the
clean-sandbox control. Its absence from scoped evidence remains visible even
though other true blockers retain the case refusal.

## What the other reasons reveal

PR2647 adds a review-record outcome display. Its grant rejection, identity
fallback, reviewer launch, release-grant stamp and hold-release decisions are
unchanged. The new `record_outcome` reads the existing recording key and rounds
for display; its output does not control release. Those known contracts are
incidental despite sharing the changed review function. But the unchanged
`EPIC-1-001` file-header trace has no resolved frozen requirement. Full-file
mode also includes unresolved `FR-0259` at the unchanged scaffold-pruning helper.
Unrelated-looking locations cannot establish the scope of an unknown contract.

PR2701 changes relay-owner liveness and stall reporting. The pickup, driver
installation, redrive and mail/cold-boot paragraphs are unchanged. Its remaining
`TASK-176` trace immediately precedes the substrate command list whose relay
description changes. The frozen current object resolves this alias to
`TASK-1-152`, a CLI-manual preflight-guard task, but the policy census could not
read history under `TASK-176`. This trace/contract mismatch stays unresolved.

PR2685 directly changes reconciliation evidence queries, open-change matching
and the diagnostic count, plus its fixture's requested fields. The frozen
current alias resolves `TASK-1335` to `TASK-1-226`, whose description concerns
unavailable reconciliation evidence. That supports a behavior touch; it does
not repair the census's unavailable historical supervision classification.
The row therefore keeps `judgment: unresolved`, separately recording the known
behavior touch. Resolving a current alias is not a new policy decision.

PR2654 changes unsupported substrate-job handling in the registry and tick,
the shared installer binary selection, and driver diagnostics. These touch
`STORY-1226`, `STORY-1218` and `TASK-1491`. The pinned installer chain is
`shift.rs:3541` → `schedule_driver::real_tick_invocation` → the changed stable
binary selector. `TASK-1492` marks the unchanged mail-episode collector; the
new unsupported-command handling concerns substrate jobs, while its redrive,
mail escalation and seat cold-boot mechanisms do not change.

PR2669 changes canonical comparison of driver binaries, documents repeated
full-tick failure for unsupported jobs, and strengthens the shared shift
installer test. Retain `TASK-1491` and `STORY-1226`, plus full-file-only
`STORY-1218`: its installer acceptance is exercised by the changed test and
shared driver chain. The pickup and mail/cold-boot contracts remain incidental.
`TASK-176` remains unresolved for the same frozen-history mismatch as PR2701.

PR2668 changes only the check-ignore precedence memory. There are no trace IDs
or named blockers, so this records the allowed baseline rather than discovering
a false-positive refusal.

PR2681 changes the stop command's partition into confirmed, recycled, unknown
and corrupt identities, keeps unconfirmed live processes unsignalled, and adds
missing/garbage identity tests plus trap-ready synchronization. Both the parent
identity-safe stop bug and the named follow-up task are true touches.

PR2686 changes `roots::survivors` to exempt only the scorer's own pre-exec
forks, plus guard test synchronization and oracle-read diagnostics. The result
feeds `record.survivors_detected`, `installer_survivors` and `record.clean` in
unchanged `score.rs`; unchanged `controls.rs` requires zero survivors for clean
controls. Retain both TASK-1730 and full-file-only TASK-1731. The unchanged
copy/link helpers carrying the latter trace are not the behavior dependency.
The changed `ShiftLock::drop` explicitly unlocks the shared flock: both
`run_tick` and `run_redrive_pass_with` hold it, so STORY-1218 and TASK-1492
are touched. Driver installation/status and the global-queue advisor grant
fixture are unchanged, making TASK-1491 and STORY-1473 incidental. The real
component bugs do not resolve the combined named branch token.

PR2653 moves exact identity reading into canonical liveness, dispatches
comparisons by record format and introduces strict positive matches for signal
paths. The cache re-export preserves the exact reader's behavior; the contained
cache read-only sidecar exception is unchanged. TASK-1750 and the drain stop
contract STORY-1130 are true touches. BUG-1929's contained-write contract and
STORY-771's earlier pure-movement handler extraction are incidental. The
ordinary named TASK-1768 is also directly implemented by strict signal calls
and refusal tests; ordinary policy status does not mean behavior is untouched.

PR2698 extracts lease/companion cleanup into `release_lease_files`, preserving
existing reap behavior, and calls it after successful deleted-worktree recovery.
That new caller revokes the old lease's delegated headless grant before removing
the lease: BUG-2139 is a true authority-lifecycle touch, despite the incidental
selected BUG-1533 sender-identity witness. The mailbox sender, queue dispatch
gates, reap/notify/chain criteria, night-shift loop, driver, mail/cold-boot logic
and pickup path resolver are unchanged. TASK-176 retains the frozen-history
uncertainty. The new direct-worktree recovery does not establish a change to
the unchanged pickup/delegation paragraph beside it.

## Evidence and limits

The data pins each base/head, every changed path's diff hash, each blocker’s
policy reason and mode membership, exact trace lines and blob hashes, and the
shared installer call and scorer/control consumer lines outside the changed
paths. Scoped occurrence selection
comes from the prior exact collector's per-path provenance. Full-file-only
occurrences are recovered from committed changed-path blobs. A full-file
occurrence can be unselected on one side even when its ID is selected elsewhere.

The policy census covers all 2,160 unique full-set trace IDs, including 331
blocking IDs; this document manually inspects only the ten cases above. It
reuses that census rather than rerunning history classification. The frozen
store is `a5dae7145abd11d2480271c466030b12db6e7668`, and the collector SHA-256 is
`7041c86bc20415191b8814875bdfc77e9aa6e8e904aea8894b9e64724e099dd8`. Input census
and provenance hashes and the unchanged private-ledger digest are in the data.
No ledger contents are published.

`all_policy_blockers_inspected` means every blocker received a manual judgment;
it does **not** mean every blocker was resolved. `all_blockers_resolved` is
separate and includes named uncertainty when a named inventory is present.
`all_exclusion_reasons_inspected` on the four new rows records complete trace
and named inspection, including unresolved tokens; it grants no policy clearance.
None of these fields grants completion, review or shipping authority.
No symlink, binary, gitlink, fallback or path-class authority change is included.
The advisor's frozen-50 measurement and 100% true-touch-retention gates remain.
