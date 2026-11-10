# STORY-2695: dell1 parallel cached CI sketch

<!-- trace:STORY-2695 | ai:codex -->

Status: docs-only sketch rework under the binding 2026-11-10 09:05 UTC
advisor ruling, which supersedes the earlier brief and conflicting story scope. No runner,
workflow, credentials or repository settings change in this slice.
Publication title: `[AI:codex] docs(ci): sketch dell1 parallel cached runner (Refs STORY-2695)`.

## Evidence and objective

The story records smoke run 42650619081 (2026-11-09), followed by routing being
turned off. These are reported measurements, not reproduced by this sketch:

| Heavy shard | dell1 seconds | Six hosted runs, seconds |
| --- | ---: | ---: |
| cli-0 | 275 | 407–595 |
| cli-1 | 236 | 375–492 |
| lib-0 | 784 | 762–1116 |
| lib-1 | 459 | 731–1158 |

The four serial dell1 jobs totaled 29.5 minutes; hosted median queue time was
378 seconds per heavy shard. Lib compilation was about 270 seconds per shard
(36% of lib-0, 58% of lib-1). The advisor's end-to-end train baseline is
40–50 minutes of serial hosted CI. Optimize that elapsed interval, including
queueing, other required jobs and aggregate settlement, rather than only
compiler speed. The smoke's failed tests prevent treating its times as green
acceptance results.

Local source evidence:

- [one-job.sh](../../scripts/runner/one-job.sh) has one global flock, fixed
  `current` directory/unit, a fresh HOME, cgroup cleanup and no local persistence.
  It reapplies CPUQuota=800% and AllowedCPUs=0-7 on every launch.
- [rearm.sh](../../scripts/runner/rearm.sh) is an operator foreground loop:
  generate one JIT config, pipe it to the installed root launcher, repeat.
  Nonzero exits or lifetimes under ten seconds stop registration churn.
- [aida-ci.slice](../../scripts/runner/aida-ci.slice) shares eight CPUs;
  it currently has **no memory limit**.
- [ci.yml](../../.github/workflows/ci.yml) has seven parallel matrix entries,
  four heavy shards eligible for dell1, a 40-minute job limit, and the hosted
  required `Build (ubuntu-latest)` aggregate. Current routing admits pushes
  and same-repository PRs only; merge groups currently stay hosted.
  Swatinem/rust-cache uses lib/cli/rest/shared groups and saves all but contracts;
  that configuration alone does not establish the proposed trust split.
- [dell1 runbook](../internal/testing/dell1-runner.md) documents the sandbox,
  operator credential boundary, namespace prerequisites, routing switch and
  absence of automatic failover. Its single-slot/no-local-survivors rules will
  need explicit replacement in the implementation.

## Slot model and resource envelope

Start with **two** slots, s0 and s1; admit s2 only after measured acceptance.
Each gets `/var/lib/aida-ci-jobs/sN/current`, unit `aida-ci-sN`, root-owned lock
`/run/aida-ci-sN.lock`, and its own operator rearm loop. The installed launcher
accepts only an enumerated slot, never a caller-supplied path or command.
Update the exact sudoers argument rules accordingly. Preserve parent mode
0755, private job root 0700, verified root-owned distribution and fresh JIT
configuration per job. Stop/reconcile only that slot's service before reuse;
refuse on stop failure. Cleanup kills its entire cgroup before deleting state.

Use a distinct unprivileged uid per slot and v1 trust class (TRAIN, MAIN),
with no supplementary groups or sudo. Do not merely give parallel jobs the
same uid and separate directory names: host networking and same-uid access
would undermine isolation. Hide sibling job/cache roots from each unit and
exercise cross-slot file, signal and proc access denials. Stop all descendants
before switching a slot's class. No registration credential enters any unit.
Ordinary same-repository and fork PRs stay hosted; a PR uid/class is explicitly
deferred beyond v1.

Keep the shared slice's CPUQuota=800%, AllowedCPUs=0-7; the other twelve cores
remain outside CI's cpuset for host work, including up to eight concurrent
Codex cold-build lanes. Two active units each get CPUQuota=400% and
CARGO_BUILD_JOBS=4, with equal CPUWeight. For three, use 266% and build jobs=2;
that is a capacity experiment, not a promised speedup. Bound libtest parallelism
too; record the setting with every timing sample.

Proposed shared memory budget: MemoryHigh=20G, MemoryMax=24G, MemorySwapMax=0;
two slots each MemoryHigh=10G/MemoryMax=12G, or three at 6G/8G.
These are proposed limits, not measured host capacity. Operator must first
measure CI together with up to eight concurrent Codex cold-build lanes,
including concurrent linking, and verify physical RAM and at least 8G remaining
host headroom after that combined peak. Reserve CPU and memory headroom for
those lanes and host services before accepting CI limits; otherwise retain
hosted routing and revise the budget. Capture memory.peak, memory.events, CPU
throttling and disk pressure for the combined load, not CI in isolation.
No memory-limit or OOM failure is accepted as a test failure to suppress.

GitHub routing uses `[self-hosted, Linux, X64, aida-dell1]` for lib-0,
lib-1, cli-0 and cli-1, additionally selecting a TRAIN or MAIN label. V1 admits
only TRAIN and MAIN: TRAIN runs are `pull_request` runs on host-created
`train/` branches; MAIN runs are main pushes at `refs/heads/main` in the expected
repository. TRAIN admission must match repository, PR number, exact head SHA
and branch against the host train record. Neither `merge_group` nor a branch
name alone establishes TRAIN eligibility. Ordinary same-repository PRs and
fork PRs (including approved forks) stay on ubuntu-latest; their PR routing
class is explicitly deferred. rest/gates/contracts and the aggregate stay
hosted. Preserve seven-lane success requirements and job timeouts. Both v1
classes share the two-slot cap; reserve train priority through host admission,
since labels alone provide no queue priority or trust proof.

## Cache trust and lifecycle

A job's YAML, environment, cache key and requested label are untrusted inputs
for deciding cache access. Before trusted dispatch, a host-side controller must
bind the GitHub run/job identity, event, repository, ref and exact checkout SHA
to a slot using independently fetched forge evidence. Refuse ambiguous or
mismatched evidence. For TRAIN, independently match the repository, PR number,
exact head SHA and branch to the host train record before assigning a slot;
a `train/` prefix alone or a `merge_group` event is insufficient. Reviewed
default-branch policy controls classification;
a PR editing its workflow cannot promote itself. This controller/admission
boundary is required implementation work, not something the existing JIT pipe
already provides. Registration still runs only in the operator's session.

| Cache | Writer | Readers / persistence |
| --- | --- | --- |
| PR cache/class (deferred beyond v1) | No v1 local writer | Ordinary same-repository/fork PR CI remains hosted; no local PR cache |
| Trusted main snapshot, per slot and shard family | Successful verified main-push job, then host publisher | Main/train receive private copies; root-owned published generations |
| Main working target | One admitted main job | Private writable copy of trusted generation; publish only after success |
| Train working target | One admitted train job | Private writable copy of trusted generation; delete after job, never publish |

Only main pushes may produce trusted generations. Host publisher copies after
unit termination into a new generation and atomically updates a root-owned
manifest; no writable trusted cache is ever mounted into a PR or train unit.
Avoid hardlinks: reflink/copy or a correctly isolated copy-on-write snapshot
must prevent writes reaching the seed. Reject symlinks/special files during
host traversal; never execute cached programs on the host. A failed main build
leaves the previous generation intact. No credential or runner configuration
belongs in any cache.

A train compiles its not-yet-landed code in its disposable copy. Its incremental
outputs die with that copy, even if tests succeed; the later actual main push
recompiles the delta and alone can advance the trusted snapshot. Main also
starts from a trusted copy, never a previous PR/train slot directory. Persist
Cargo registry/git downloads under the same class rules, not a common writable
CARGO_HOME. Disable remote Rust-cache restore/save for these local trusted
paths until an equally enforced publisher exists; no fallback to PR cache keys.

Preserve a stable absolute workspace/HOME/target path per slot and class using
private mount views. Key generations by slot, shard family, rustc -Vv, target,
profile/features, Cargo.lock and reviewed build configuration. Persist target's
incremental data and workspace artifacts explicitly; merely retaining a
third-party dependency archive does not remove the measured lib compilation.
A key mismatch starts cold. Record seed main SHA and age; missing seed builds
clean. Check out the exact admitted main/train SHA independently of cache
contents; cached Git metadata or config must never choose the source revision.

Acceptance must prove ordinary same-repository and fork PR jobs remain hosted,
including PR-edited routing YAML and forged TRAIN labels. Reject missing or
mismatched host train records (repository, PR, exact head SHA or branch), and
branch-name-only or merge-group claims. Local PR class/cache acceptance is
deferred beyond v1. Attempt direct/alias/symlink writes to trusted roots and
sibling slots; prove MAIN/TRAIN builds use only the recorded main generation.
Also mutate a train's target and prove trusted hashes stay unchanged. Verify
positive private-cache writes, cache misses and clean-build equivalence.
A workflow `save-if` condition alone cannot satisfy this boundary.

## Seven environmental failures: investigation and repair plan

No raw failure logs were supplied beyond the story's environmental summary.
The following are source-grounded hypotheses, not established diagnoses.
Capture exact failing operation, errno, unit properties and relevant audit
denials first, under the installed unit. Keep AIDA_REQUIRE_BWRAP_LIVE=1,
positive controls, denial assertions and all seven executed tests.

| Test (module prefix from story) | Hypothesis and proposed repair |
| --- | --- |
| contained_boundary::bug_2418_task_2772_lane_cargo_tests::lane_build_paths_are_only_the_slot_pair | Fixture hardcodes `/home/u` and `/run/user/1000`; the helper resolves existing ancestors, while ProtectHome hides those roots. Replace fixture paths with private temp HOME/runtime directories and assert exactly the derived slot pair; retain canonicalization and sccache exclusion checks. |
| same module: lane_build_paths_never_reach_cargo_home | Same hidden ancestors can make resolution refuse before overlap assertions. Use real temporary paths, including relative and symlink cases; retain every CARGO_HOME ancestor/descendant refusal. Do not expose operator HOME. |
| seat_clone::lifecycle_tests::task_2053_bwrap_probe_runs_no_seat_filter_and_reads_only_the_seat_view | `outside_file()` uses compile-time CARGO_MANIFEST_DIR. A restored binary can name another slot's vanished checkout; `/var/lib` placement may also differ from the sandbox's expected visible roots. Use a controlled outside-seat fixture in the test's accessible temp tree, and retain readable unfiltered control, malicious-filter nonexecution and narrowed-view denial. Check nested mount stderr before changing the unit. |
| gate::mandate::story_1698_mandate_tests::exclusion_writer_refuses_invalid_ledger_without_appending | Test chmods to 04600; RestrictSUIDSGID=yes can deny setting that bit before ledger validation. Keep the production unit restriction. Move the privileged-bit metadata case to a factored metadata predicate fixture; retain actual-file 0644 refusal, malformed-content refusal and byte-for-byte no-append checks. Require an actual 04600 integration fixture in a separate operator-prepared restricted test unit if needed; never silently omit it. |
| same module: task_1790_exclusions_require_private_owned_regular_file | Same 04600 chmod issue; modes 0644/0660/0400 and uid mismatch still need coverage. Factor mode validation so 04600 is always checked without requiring the harness to create a setuid file, plus real-file tests for allowed chmods. Preserve exact 0600, regular-file and owner requirements; diagnose any earlier read error separately. |
| regen::score::tests::confined_environment_hides_original_source_and_refuses_exposed_system_roots | Compile-time source path may be stale across cached slots; nested bwrap mount policy can also yield EPERM. Use a real controlled source with `.git` and a separate trial root; retain hidden-source success, deliberately exposed source refusal and writable isolated HOME/XDG controls. Probe actual fresh proc/dev and nested userns under the unit; do not use --unconfined. |
| launch::contained::tests::task_2708_live_host_udp_named_send_refused | Host-FD positive control must bind UDP and pass SCM_RIGHTS before testing EACCES. Check AppArmor/nested-userns logs and outer syscall restrictions, not just bwrap availability. If policy blocks namespace setup, operator supplies a narrowly reviewed runner policy or disposable VM. Keep PrivateIPC, socket transfer control, exact EACCES named-send denial and no received bypass datagram; do not replace EACCES with arbitrary EPERM success. |

The runbook already warns against ProtectProc/ProtectKernelTunables because
proc overmounts break nested mounts. Preserve that constraint, NoNewPrivileges,
ProtectHome, read-only system and blocked host-control sockets. Never unload
host AppArmor as CI does on disposable hosted VMs. If a repair requires broader
host exposure, keep routing off pending independent review or use disposable
VMs. Run each regression, then all four heavy shards concurrently, with executed
test counts and positive-control output; an availability skip is not green.

## Binding delivery slices

- **S1 — environmental repairs only.** Capture the exact seven failures under
  the installed unit properties, including operations, errno and positive
  controls, then repair all seven environment tests using the plan above.
  No runner, workflow or routing change belongs in S1. Preserve containment
  assertions and execute the repaired regressions; no weakened or skipped test
  counts as acceptance. Any required runner-policy change remains for S2.
- **S2 — runner implementation.** Own two slots, cache trust/lifecycle,
  host-record-bound TRAIN/MAIN admission, combined CI/eight-lane CPU and memory
  budgets, and the updated runbook. Supply isolation, cache and admission
  evidence while routing stays off; PR class/cache remains deferred.
- **S3 — operator rollout and completion.** Own operator installation,
  activation and the matched ten-train measurement described below. Only S3
  completes STORY-2695 after acceptance; this sketch, S1 and S2 retain Refs
  credit. Registration credentials remain an operator act outside the lane.

## Latency forecast, rollout and rollback

Optimistic warm lib estimates subtract the entire measured 270s compile:
lib-0 514s (8.6m), lib-1 189s (3.2m). These are lower bounds, not forecasts of
perfect reuse. Budget warm lib-0 9–12m, lib-1 4–7m, cli-0 4–6m and cli-1 4–6m;
no measured CLI compile split supports a stronger claim. Cold isolated times
remain approximately 13.1m/7.7m/4.6m/3.9m, before parallel CPU contention.

With two slots, four cold jobs total 29.2m of recorded service time; ideal
balanced packing is about 16m. Warm budgets imply roughly 13–18m heavy-job
makespan with favorable ordering. These estimates assume useful overlap at
four cores each; the original smoke shared eight cores with only one job.
Measure contention: doubling compile time could erase much of the gain.
For a single train, target **20–30m end-to-end** versus 40–50m baseline,
including registration/queue and hosted gates; cold runs may take 30–40m.
Three slots are optional only if measured train p95 improves within the same
8-core/24G budget. More slots cannot remove serial trains or hosted gate delay.

Keep AIDA_DELL1_CI unset throughout implementation and independent review.
Operator then verifies memory/disk capacity, installs reviewed scripts/units,
starts two foreground rearm loops and performs namespace, isolation, cache
poisoning and all-shard smoke acceptance. Never generate JIT credentials here.
Enable AIDA_DELL1_CI=true only after those gates, initially for a bounded window.
Compare at least ten comparable green trains with a matched hosted window:
record run start through aggregate finish, per-shard created/started/finished,
queue median/p95, seed SHA/hit, cold/warm state, runner/slot, peak memory and
concurrent Codex lane count and cold-build load. Include the combined-load
capacity gate above before accepting limits; report other hosted jobs as
possible critical paths. S3 owns this measurement and completion decision.

Rollback immediately on any trust-boundary failure, containment skip/failure,
OOM, cleanup overlap or repeated rearm failure; also roll back if matched median
train duration or heavy-shard queue time exceeds baseline, or p95 worsens by
more than 20%. Unset AIDA_DELL1_CI, cancel queued self-hosted runs and start
fresh hosted runs (reruns may retain routing); stop operator loops, quarantine
cache generations and retain diagnostics. There is no automatic hosted failover.
