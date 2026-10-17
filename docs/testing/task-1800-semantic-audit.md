# Frozen-50 semantic audit: eighteen complete blocker inventories

<!-- trace:TASK-1800 | ai:codex -->

Eighteen cases now have manual inspection of **every policy-blocking trace ID**, in
both full-file and scoped modes. This is an incremental measurement: **32 cases
remain**, and the classification gate is open. No refused case inspected so
far is proven incidental-only. PR2668 remains the frozen allowed case, with no
blocking trace IDs. No collector or policy behavior changes.

The [machine data](task-1800-semantic-audit.json) extends the
[selected-witness audit](task-1800-witness-audit.md). It inventories all 194
full-file case/ID pairs (161 scoped pairs), not just the first refusal witness:

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
| 2656 | 12 / 9 | None established | ADR-54, BUG-1688, BUG-1693, BUG-1924, BUG-2014, STORY-1382, STORY-612, TASK-1205 | TASK-1363, TASK-1364†, TASK-1365†, TASK-1366† |
| 2674 | 14 / 14 | None established | ADR-54, BUG-1688, BUG-1693, BUG-1924, BUG-2014, EPIC-48, STORY-1353, STORY-1382, STORY-564, STORY-612, TASK-1205, TASK-1289 | EPIC-1-001, TASK-1363 |
| 2697 | 17 / 17 | TASK-963 | BUG-1167, BUG-1953, BUG-710, BUG-716, BUG-727, STORY-1166, STORY-1171, STORY-1698, STORY-771, TASK-1277, TASK-1330, TASK-1600 | TASK-1331, TASK-140, TASK-141, TASK-142 |
| 2676 | 22 / 14 | BUG-2148, TASK-1750 | BUG-1043, BUG-1156, BUG-1178†, BUG-1699†, BUG-1983, BUG-2005, BUG-2006†, BUG-2124†, BUG-2133, BUG-2144, BUG-2145, BUG-2146, STORY-1124†, STORY-1473, STORY-561, STORY-564, STORY-994, TASK-1232†, TASK-1233†, TASK-798† | None |
| 2672 | 18 / 18 | BUG-1178, BUG-1699, BUG-2006, BUG-2124, BUG-2133, BUG-2144, BUG-2146, BUG-2151, TASK-1232 | BUG-1720, BUG-2145, STORY-1124, STORY-1226, STORY-561, TASK-1607, TASK-1750, TASK-864 | FR-1-043 |
| 2670 | 31 / 15 | BUG-1929, TASK-1775 | BUG-1164, BUG-1593†, BUG-1928, BUG-1971, BUG-2006, BUG-2133†, BUG-2144†, BUG-2146†, BUG-675, STORY-605, STORY-612†, STORY-615†, STORY-617†, STORY-665, STORY-994, TASK-1169†, TASK-1594†, TASK-845†, TASK-864†, TASK-866†, TASK-876†, TASK-951 | BUG-1-040, CR-8, FR-1-002, FR-1-043, FR-1-044†, TASK-152†, TASK-153† |
| 2652 | 18 / 18 | None established | BUG-1164, BUG-1535, BUG-1929, BUG-2003, BUG-2009, BUG-715, BUG-724, BUG-725, FR-265, STORY-1226, STORY-652, STORY-654, TASK-1122 | BUG-1-051, BUG-1-069, EPIC-1-052, FR-1-071, FR-1-074 |
| 2666 | 19 / 19 | TASK-1730, TASK-1731 | BUG-1693, BUG-1806, BUG-1929, BUG-2011, BUG-2014, STORY-1687, TASK-1337, TASK-1600, TASK-1607 | FR-0042, FR-1-052, TASK-123, TASK-1327, TASK-1328, TASK-1331, TASK-1335, TASK-1338 |

† Full-file mode only. Trace totals are **31 true supervised touches, 126
incidental matches, 37 unresolved** in full-file mode; scoped totals are
**29, 102, 30**. Counts are case/ID pairs, so a requirement present in two cases
counts twice. Five cases have no unresolved exclusion reason: PR2654, PR2681,
PR2653 and PR2676 have true supervised touches; PR2668 has no blockers and was
already allowed. The other thirteen retain unresolved evidence, including the combined
named token in PR2686. The frozen conservative outcome remains **49/50
refusals**, comprising 22 named refusals and 27 additional trace refusals.

Twelve inspected cases also inventory **every extracted named token**:

| PR | Named tokens | Frozen named result and manual finding |
| --- | --- | --- |
| 2668 | TASK-1767 | Allowed; changed precedence memory directly implements the ordinary task |
| 2681 | TASK-1782 | Identity metadata refusal; directly touched |
| 2686 | BUG-2160, BUG-2160-2161, BUG-2161 | Component bugs directly touched and ordinary in the frozen all-ID census; combined branch token remains unresolved |
| 2653 | TASK-1750, TASK-1768 | Keystone TASK-1750 directly touched; TASK-1768 also directly touched and ordinary in the frozen all-ID census |
| 2656 | BUG-2135 | Ordinary; compiler-cache detection, startup/restart, hints and fixtures directly implement it |
| 2674 | TASK-1780 | Ordinary; local testing tiers, targeted-test planner and Makefile target directly implement it |
| 2697 | BUG-2164 | Ordinary; default-branch/no-number refusal and approved-PR suggestions directly implement it |
| 2676 | BUG-2148 | Keystone refusal; start-identity capture and strict stop/teardown signal checks directly implement it |
| 2672 | BUG-2144, BUG-2145, BUG-2146, BUG-2151 | All named keystone blockers; BUG-2144/2146/2151 directly touched, BUG-2145 incidental (queue auto-complete/sandbox dispatch unchanged) |
| 2670 | TASK-1775 | Keystone refusal; store marker ordering, hardened staged diff and expiring host-version cache directly implement it |
| 2652 | BUG-2141 | Ordinary; anchored shared runtime exclude, legacy migration and Git ignore/staging fixtures directly implement it |
| 2666 | TASK-1732 | Ordinary; strict mapping capture, R1/R2 environment transport and fresh execution, adapter evidence/deltas and fixtures directly implement it |

These named findings are separate from the trace-pair totals. The earlier five
cases and PR2698 had no named refusal. PR2656, PR2674 and PR2697 also had no
named refusal; their ordinary named tokens
are inventoried for completeness. PR2676 and PR2672 have named keystone refusals
and no unresolved named token. PR2670 also has a named keystone refusal; PR2652
has no named refusal. PR2666 also has no named refusal; its ordinary TASK-1732 is directly touched.
None of these new named tokens is unresolved. No policy/history
classification was rerun.

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

PR2656 adds compiler-cache process/temp-directory detection, failure hints and
sccache startup/restart support. Its workflow change adds one fixture step;
the GitLab authentication guard, shipped launch refusal, merge-hold fixtures,
role/grant banner, expiry hook, prose ratchet, live bwrap and portability steps
are unchanged. The local runner starts sccache before gates but retains their
command bodies and result handling. The new Rust `ensure_sccache_server` helper
has no caller in this pinned revision; `run_checked` gains failure diagnostics.
No recovery helper or crash-matrix code changes. TASK-1363 through TASK-1366
remain unresolved: the frozen current aliases resolve to TASK-1-294,
TASK-1-297, TASK-1-295 and TASK-1-296 respectively, but the frozen policy census
could not read their history. Three are full-file-only. Apparent separation
from their known current recovery/zsh contracts does not repair that history.

PR2674 adds a targeted cargo test planner, a Makefile target, one CI planner
fixture step and local testing-tier instructions in discipline/template masters.
The planner scans Rust/build inputs to select cargo tests; it does not change
runtime authority. The existing live bwrap, merge-hold, grant notice/banner,
launch-refusal and ratchet fixture commands remain unchanged. Likewise the
actual dispatch/disposition boundary, zen clean/needs-human finish sequence,
research ablation findings and parent-owned preflight/publication paragraph
remain unchanged. Their traces are incidental. Unresolved EPIC-1-001 occurs
in an unchanged example warning against inaccurate traces; its example-like
placement cannot resolve an unknown contract. TASK-1363 retains the same
frozen-history uncertainty as PR2656.

PR2697 adds a default-branch/no-explicit-number refusal and prints open approved
PR suggestions. The later hold/mandate, self-merge, supervision, serialization,
lease cleanup, protocol, identity sanitization and completion-trailer mechanisms
are unchanged. Suggestion titles and review decisions feed only the refusal
message, not permission or completion. However, `find_open_approved_prs` adds a
new forge-sensitive metadata read: GitHub directly runs `gh pr list --json
number,title,reviewDecision`; other forges receive no suggestions. Retain
TASK-963 as a true touch to the PR metadata-read/Forge consolidation contract,
even though the existing `fetch_change_info_via_forge` callsites are unchanged.
This is a portability-contract touch, not a claim that suggestions authorize
merging. CI-watch/check-run abstraction STORY-1166 is a separate unchanged
contract. TASK-1331, TASK-140, TASK-141 and TASK-142 remain unresolved because
the census lacked history under those names; current aliases TASK-1-223,
TASK-1-115, TASK-1-117 and TASK-1-118 do not repair it. In particular, feature
branch resumption is unchanged; default-branch no-number invocation is refused.

PR2676 adds recorded start identity to registry/lease views, captures leaked
session identities and checks strict matches before TERM and again before KILL.
BUG-2148 and TASK-1750 are true touches. The unchanged canonical registry-live
predicate still distinguishes role-enter shells from their descendant sessions;
this PR does not alter BUG-1156's liveness/reaping predicate. MCP authority,
parent-grant fixtures, containment launch fixtures, terminal capture/title,
presence, wrapper staleness and the explicit zen finish outcome remain unchanged.
Its extra `pid_start` fixture fields are process identity, not seat or terminal
authority. All 22 full and 14 scoped blockers are resolved; the refusal is true.

PR2672 changes contained-parent posture selection, including resumed Claude's
own worktree and background config-contained launches. BUG-2151 is direct.
BUG-1178's interactive posture selection is touched even though the promptable
flag generator and dontAsk guard remain unchanged. BUG-1699 is touched by adding
`--allow-dangerously-skip-permissions` to the posture partition. New/resume
`config.contained` inputs feed unchanged environment consumers, touching BUG-2006
and BUG-2124. A new fallible background config read touches BUG-2146. Both final
guards precede `noexec`, changing the preview entry behavior and accepted argv
inputs, so TASK-1232 is also retained.

The same PR leaves bypass detection/stripping, phase-driver child builders,
queue-work containment dispatch, Codex-native workspace-write flags, due-job
rendering, presence, seat gateways and canonical process liveness unchanged.
However, BUG-2133's frozen advisor ruling explicitly says no child may run with
less confinement than its parent; BUG-2144's frozen cap ruling names BUG-2151 as
its agent-new continuation. Both are true shared-contract touches despite their
unchanged trace-local code. BUG-2145 is incidental in both trace and named
inventories: its queue auto-complete/sandbox flag path does not change.
The actual OS-wrap call and `os_wrap_enabled(worktree_root)` decision are
unchanged: config containment is not that decision input. TASK-864 is therefore
incidental despite sharing the launcher. FR-1-043 remains unresolved; its
unchanged session-command dispatcher trace has no frozen contract.

PR2670 moves the contained-store marker before the host Git version check, routes
`auto_commit_paths`' staged-diff predicate through `host_git_command`, extends
the raw-store-Git guard to `git_backend.rs`, and expires the host-version cache
after 60 seconds. TASK-1775 is directly implemented. BUG-1929's frozen operator
round-4 ruling requires all host store Git calls to use the hardened builder;
round 8 requires attributes hardening. That shared contract is a true touch,
including the changed version-cache input, despite unchanged hook pinning and
writable roots. BUG-1164's staging/lock recovery is unchanged: only the later
diff predicate changes. BUG-1928's permission rules, BUG-1971's root selection,
child set/strip and monotonic-posture decisions, vendor/config refusals, network
settings and OS-wrapper behavior are unchanged. The frozen BUG-1928 triage
explicitly separates its permission layer from BUG-1929's store Bash boundary.
The broader BUG-2133 child-confinement ruling does not turn this host-store
change into a child posture change. Identity, terminal, setup, grant and wait
ceiling contracts are also unchanged. Seven blockers remain unresolved:
BUG-1-040, FR-1-002/043/044 have no frozen contract; current aliases
CR-8 → CR-1-173, TASK-152 → TASK-1-128 and TASK-153 → TASK-1-129 do not repair
the census's missing history. The named refusal is true; the case is not fully
resolved.

PR2652 anchors the runtime exclusion to `/.aida/*`, migrates legacy `.aida/`
lines in the common Git exclude file and verifies template/discipline/config
addability while runtime files stay ignored. Ordinary named BUG-2141 is direct.
All 13 known trace-blocking contracts are incidental. In particular,
`ensure_store_lock_excluded` still supplies the same lock pattern through
`append_exclude_entries`; the new delegate receives an empty migration list,
so this shared store path preserves its behavior. Host Git hardening and hook
refresh/modes, stale-index-lock recovery, queue CRDT merge, scheduler ledger,
node/owner identity, public redaction and short-ID collision handling do not
change. New fixtures' generic Git identities do not implement production node
identity or redaction. Five unknown contracts remain unresolved: BUG-1-051,
BUG-1-069, EPIC-1-052, FR-1-071 and FR-1-074. Their unchanged trace locations
are not enough to establish safety. No refused PR is proven incidental-only.

PR2666 adds strict trial-root GAPS mapping, one-time capture of explicitly named
source variables, R1 transport, and fresh R2 execution with target aliases.
TASK-1730 is a true touch: `ScoreEnv::run_with_bundle` changes the environment delivered
to the oracle, `run_trial_in_env` sanitizes mapped output, and the versioned
trial record retains R2 evidence. TASK-1731 is also true: the existing gate call
moves into `adapter::apply_gate`, both runs receive it, and combined status and
cleanliness prevent a refused R2 from hiding behind R1. The new control fixture
covers guard hits, changed checks, installer failure, survivors and the signed
nonzero-oracle policy. Unchanged predicate bodies do not make their new callers
incidental. Ordinary named TASK-1732 directly implements its frozen adapter and
transport rulings.

The same PR leaves hold clearance/tamper detection, shell role-eval framing,
contained store-write roots and host Git/hook hardening, grant verdicts/expiry,
Claude notices, pickup preflight, completion credit and seat occupancy unchanged.
The existing persistence seam receives new scoring evidence without changing
store-write authority or its failure handling. These nine known trace-blocking
contracts are incidental. Eight blockers remain unresolved: FR-0042 and
FR-1-052 have no frozen objects; current aliases TASK-123 → TASK-1-096,
TASK-1327 → TASK-1-219, TASK-1328 → TASK-1-220, TASK-1331 → TASK-1-223,
TASK-1335 → TASK-1-226 and TASK-1338 → TASK-1-229 do not repair unavailable
historical supervision. Example traces and apparently unrelated paragraphs do
not prove an unknown contract safe. A true trace refusal is established, but
this case is not fully resolved.

## Evidence and limits

The data pins each base/head, every changed path's diff hash, each blocker’s
policy reason and mode membership, exact trace lines and blob hashes, and the
shared installer call, scorer/control consumers and launcher environment/OS-wrap
decisions outside the changed hunks. The new cases also pin full frozen spec
object hashes, including the advisor comments that define shared contract scope.
Scoped occurrence selection
comes from the prior exact collector's per-path provenance. Full-file-only
occurrences are recovered from committed changed-path blobs. A full-file
occurrence can be unselected on one side even when its ID is selected elsewhere.

The policy census covers all 2,160 unique full-set trace IDs, including 331
blocking IDs; this document manually inspects only the eighteen cases above. It
reuses that census rather than rerunning history classification. The frozen
store is `a5dae7145abd11d2480271c466030b12db6e7668`, and the collector SHA-256 is
`7041c86bc20415191b8814875bdfc77e9aa6e8e904aea8894b9e64724e099dd8`. Input census
and provenance hashes and the unchanged private-ledger digest are in the data.
No ledger contents are published.

`all_policy_blockers_inspected` means every blocker received a manual judgment;
it does **not** mean every blocker was resolved. `all_blockers_resolved` is
separate and includes named uncertainty when a named inventory is present.
`all_exclusion_reasons_inspected` on the twelve named-inventory rows records complete trace
and named inspection, including unresolved tokens; it grants no policy clearance.
None of these fields grants completion, review or shipping authority.
No symlink, binary, gitlink, fallback or path-class authority change is included.
The advisor's frozen-50 measurement and 100% true-touch-retention gates remain.
