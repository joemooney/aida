# Frozen-50 semantic audit: first five complete blocker inventories

<!-- trace:TASK-1800 | ai:codex -->

Five cases now have manual inspection of **every policy-blocking trace ID**, in
both full-file and scoped modes. This is an incremental measurement: **45 cases
remain**, and the classification gate is open. None of these five proves a
whole PR incidental-only. No collector or policy behavior changes.

The [machine data](task-1800-semantic-audit.json) extends the
[selected-witness audit](task-1800-witness-audit.md). It inventories all 21
full-file case/ID pairs (18 scoped pairs), not just the first refusal witness:

| PR | Full/scoped blockers | True supervised touches | Incidental blockers | Unresolved blockers |
| --- | ---: | --- | --- | --- |
| 2647 | 6 / 4 | None established | BUG-1978, BUG-1983, STORY-1473†, STORY-1687 | EPIC-1-001, FR-0259† |
| 2701 | 4 / 4 | None established | TASK-1337, TASK-1491, TASK-1492 | TASK-176 |
| 2685 | 1 / 1 | Behavior touched; supervision unknown | None | TASK-1335 |
| 2654 | 4 / 4 | STORY-1218, STORY-1226, TASK-1491 | TASK-1492 | None |
| 2669 | 6 / 5 | STORY-1218†, STORY-1226, TASK-1491 | TASK-1337, TASK-1492 | TASK-176 |

† Full-file mode only. Full-file totals are **6 true supervised touches, 10
incidental matches, 5 unresolved**; scoped totals are **5, 9, 4**. Counts are
case/ID pairs, so a requirement present in two cases counts twice. PR2654 has
no unresolved blockers and has true supervised touches. The other four retain
unresolved blockers; PR2669 also has true supervised touches. These results
cannot justify admitting any of the five PRs or changing the frozen 49/50
conservative refusal count.

PR2669's true parent touch, `STORY-1218`, is absent from the existing scoped ID
set even though other true blockers still refuse that PR. Case-level refusal
retention therefore does not prove retention of every true touched ID. Future
coverage checks must preserve this distinction; this measurement repairs no
collector gap.

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

## Evidence and limits

The data pins each base/head, every changed path's diff hash, each blocker’s
policy reason and mode membership, exact trace lines and blob hashes, and the
shared installer call outside the changed paths. Scoped occurrence selection
comes from the prior exact collector's per-path provenance. Full-file-only
occurrences are recovered from committed changed-path blobs. A full-file
occurrence can be unselected on one side even when its ID is selected elsewhere.

The policy census covers all 2,160 unique full-set trace IDs, including 331
blocking IDs; this document manually inspects only the five cases above. It
reuses that census rather than rerunning history classification. The frozen
store is `a5dae7145abd11d2480271c466030b12db6e7668`, and the collector SHA-256 is
`7041c86bc20415191b8814875bdfc77e9aa6e8e904aea8894b9e64724e099dd8`. Input census
and provenance hashes and the unchanged private-ledger digest are in the data.
No ledger contents are published.

`all_policy_blockers_inspected` means every blocker received a manual judgment;
it does **not** mean every blocker was resolved. `all_blockers_resolved` is
separate. None of these fields grants completion, review or shipping authority.
No symlink, binary, gitlink, fallback or path-class authority change is included.
The advisor's frozen-50 measurement and 100% true-touch-retention gates remain.
