# dell1 CI runner: two-slot preparation and operator acceptance

<!-- trace:STORY-2695 | ai:codex -->

S2 implements the launcher/controller and cache lifecycle. **Routing stays off:**
keep `AIDA_DELL1_CI` unset. S2 changes no workflow, repository settings, installed
units, accounts or credentials. S3 owns installation, workflow integration,
activation and ten-train acceptance. The existing workflow's same-repository PR
routing is historical and is **not** suitable for activating this controller.
The binding [plan](../../plans/2026-11-10-story-2695-dell1-parallel-cached-runner.md)
and the 2026-11-10 14:41 UTC brief supersede the single-slot runbook.

## Boundary and protocol

Only `s0` and `s1` exist. Each has a root-owned `/run/aida-ci-sN.lock`, service
`aida-ci-sN`, and private job root `/var/lib/aida-ci-jobs/sN/current` (0700).
Parents are root-owned 0755 for .NET traversal. A global admission lock reserves
run/job identities across both slots; execution uses independent slot locks.
Reservations are released after confirmed cgroup shutdown, including failed tests and delayed forge completion. Failed shutdown retains the reservation for operator reconciliation.
Stop/reconcile the previous service before reuse; failed stop or populated
cgroup refuses deletion, class switching and publication. Entire cgroups die
before cache traversal. A fast service exit or any refusal stops the operator
loop rather than churning registrations.

Four distinct locked non-root accounts are required: `aida-ci-s0-main`,
`aida-ci-s0-train`, `aida-ci-s1-main`, `aida-ci-s1-train`. Each has only its own
primary group, no sudo, no shared lane groups. Job paths are deleted between
invocations. Sibling slot roots, host policy and every trusted cache root are
inaccessible inside units. Distinct uids prevent cross-slot/class signalling,
`/proc/PID/{mem,environ,fd}` and private file access; host networking is retained.
This is a host sandbox, not a VM and not a secrets-bearing runner.

`rearm.sh sN`, in the credentialed operator's foreground session, proposes a
queued heavy job (TRAIN first) and sends a bounded JSON admission envelope on stdin through the installed launcher. The envelope has `run`, `job` (positive integers) and `runner` (`aida-ci-sN-` plus UUID hex). The installed `rearm.py` waits for the controller readiness handshake before generating JIT configuration and sending it on the next stdin line. Priority deferral exits 75 without registration; the loop retries.
Its identities, class labels and cache requests grant **no** authority.
The proposal queues are disjoint: s0 proposes lib-0/cli-1 and s1 proposes lib-1/cli-0. MAIN priority checks use this same proposal family. Actual GitHub assignment may cross families.
Registration credentials never enter the unit or root controller; only the
single-use runner configuration goes through stdin. No JIT value goes in argv
or a persistent file. Public read-only GitHub API requests run independently
in the host controller, without the operator's registration credentials. An
optional dedicated read-only forge token stays on the host. API errors,
rate limits, malformed evidence and truncated queue views refuse admission.

The root controller fetches the run/job/PR and checks repository, run attempt,
head SHA, workflow path/digest and heavy shard. MAIN is a main push; GitHub's run
API represents `refs/heads/main` as `head_branch: main`. TRAIN requires a
`pull_request` run and exactly one host record matching **repository, PR number,
exact head SHA and train/ branch**; it also checks the live PR's head/base repo,
branch and SHA. Forks, ordinary PRs, branch-prefix-only claims, ambiguous records,
merge groups and forged labels refuse. MAIN waits while host-record-matching
TRAIN heavy jobs are queued; the two classes share the same two-slot cap.

JIT labels do not bind a queued job to a runner. Therefore an immutable
`ACTIONS_RUNNER_HOOK_JOB_STARTED` connects to a host Unix socket before any
workflow step. Kernel peer uid must equal this slot/class uid. The host fetches
the uniquely assigned job by runner name across in-progress runs and applies full admission again, including repository, PR, SHA, train record and workflow digest. Source extraction, key selection and seeding then use the actual job and shard, before releasing the hook. The root-owned work directory stays inaccessible to the service uid until preparation finishes; ownership changes never follow links. A refused pickup makes the host stop the whole cgroup while the hook
stays blocked; merely returning a failed hook would allow `if: always()` steps.
Transport failure also stays blocked for operator recovery, never returning
execution to a refused job. The gate is one-shot. This is essential even with reviewed routing YAML.
The installed .sh hook invokes Python in isolated mode; a .py hook path is
unsupported by the [runner hook dispatcher](https://github.com/actions/runner/blob/main/src/Runner.Common/HostContext.cs).
Reconfirm hook-before-action/container ordering against the installed runner
version during acceptance ([job extension source](https://github.com/actions/runner/blob/main/src/Runner.Worker/JobExtension.cs)).
No PR-editable workflow or environment chooses cache trust. Both head and live
default-branch workflow bytes must match the installed reviewed policy digest.

## Cache layout and source identity

Cache generations live at
`/var/lib/aida-ci-cache/sN/SHARD/KEY/GENERATION`; root publishes `manifest.json`
with generation, key, seed main SHA and timestamp. Key inputs include slot,
shard, host-verified `rustc -Vv`, target, reviewed profile/features, Cargo.lock,
Cargo manifests/build scripts, .cargo and workflow/build configuration. Missing
or mismatched keys build cold. The controller reports seed SHA/time/age at launch.

Every job gets a byte copy of the recorded generation, with independent inodes.
MAIN and TRAIN have stable private workspace/HOME/target/CARGO_HOME paths under
`current/main` and `current/train`. The target includes workspace artifacts and
incremental output. Cargo persistence includes only registry/git downloads;
Cargo config, credentials, bin, runner configuration and HOME never persist.
The installed immutable toolchain must match policy. Reflinks could be added
later; current byte copies trade preparation time/disk space for simplicity.

Only a successful MAIN job can publish. After service termination and cgroup
emptiness, the host independently requires that the exact assigned forge job
completed successfully, copies payload into a new root-owned generation and
atomically replaces the manifest. TRAIN copies always die, including successful
trains; failed MAIN keeps the previous manifest. No trusted generation is mounted
writable or visible inside a unit. Host traversal refuses symlinks (including
replaced target/Cargo/parent directories), FIFOs and other special files. Cargo's
ordinary hardlinked artifacts are copied to independent inodes, never linked
into the seed. Cached programs never execute on the host. Copies preserve ordinary artifact
mtime/execute bits and clamp future mtimes; fresh source timestamps keep old
outputs from hiding source changes. Cargo may rerun rustc for workspace crates;
its retained incremental data, not a stale fingerprint, must supply the speedup. Root publisher copies
preserve executable bits but remove group/other permissions and special bits.

The host fetches the exact admitted SHA's public source archive, refuses archive
links/traversal, and reconstructs Git object/index identity with an independent
public fetch/reset. It never runs checkout filters/hooks, cached Git config or
cached programs. The reviewed S3 workflow must use `AIDA_CI_WORKSPACE` and
the Git HEAD in `AIDA_CI_WORKSPACE`, compile into `CARGO_TARGET_DIR`, and honor the installed pinned
Rust toolchain. It must not silently checkout a PR merge ref instead of the
admitted head. Remote rust-cache restore/save must be disabled on local paths;
no fallback to PR cache keys. Hosted jobs retain their existing behavior.

## S3 installation prerequisites (operator only)

1. Keep routing unset. Measure physical RAM and combined CI plus **eight cold
   Codex lanes**, including concurrent linking. Require at least **8G host RAM
   headroom after the combined peak**; otherwise revise limits and retain hosted
   routing. Retain `memory.peak`, `memory.events`, CPU throttling, disk pressure
   and free-space measurements. These budgets are proposed, not measured capacity.
2. Create the four accounts above with `useradd --system --user-group --shell
   /usr/sbin/nologin --home-dir /var/empty/ACCOUNT ACCOUNT`. Verify distinct uid
   and private primary gid, no supplementary groups/sudo, operator HOME 0700,
   no readable SSH/auth/grant files and Linux protected-hardlink policy enabled.
3. Install systemd, Git, curl, Python 3, jq, build-essential, pkg-config, protoc,
   bubblewrap, ripgrep, zstd, zsh, openssh-client, dbus-daemon, .NET prerequisites
   and the XCB/XKB/SSL/GTK development packages used by hosted CI. Install a
   checksum-verified unconfigured runner at `/opt/aida-actions-runner` and a
   reviewed pinned toolchain at `/opt/aida-ci-toolchain`. Entire distributions,
   files and ancestors must be root-owned, not group/other writable and contain
   no symlinks. Never install from a PR-controlled checkout. Operator home and
   registration credentials remain outside both distributions and caches.
4. Install reviewed `one-job.sh` at `/usr/local/sbin/aida-ci-one-job` (0755),
   `controller.py` at `/usr/local/libexec/aida-ci-controller.py` (0644),
   `job-gate.py` at `/usr/local/libexec/aida-ci-job-gate.py` (0644),
   `job-gate.sh` at `/usr/local/libexec/aida-ci-job-gate.sh` (0755), and
   `rearm.py` at `/usr/local/libexec/aida-ci-rearm.py` (0644), and
   `select-job.py` at `/usr/local/libexec/aida-ci-select-job.py` (0644), all
   root-owned. Install `aida-ci.slice` under `/etc/systemd/system` and reload.
5. Install root-owned `/etc/aida-ci` (0700), policy and train record JSON (0600).
   No job or rearm stdin can author these files. An operator-reviewed host train
   record export must keep `trains.json` current, atomically; absent records
   refuse. Each entry has exactly `repository`, numeric `pr`, `sha`, `branch`.
   Example: `[{"repository":"joemooney/aida","pr":123,"sha":"<40 lowercase hex>","branch":"train/example"}]`.
   Export from the host's actual `.aida/train/*.json` journals (`change_id`,
   `branch`, `train.tip`), checking the expected repository and unfinished train
   state. This is a host attestation, not an API-discovered list of train/ branches.
   For sustained API capacity, install a separate fine-grained read-only token
   at `/etc/aida-ci/forge-read-token` (root:root 0600) with only repository
   Actions/contents/PR read access, no runner registration or write authority.
   It never enters units, JIT envelopes, argv, caches or job logs. Without it,
   public unauthenticated API limits can stop loops; rate-limit errors refuse.
6. `policy.json` schema 1 requires `workflow_sha256` (SHA-256 of reviewed workflow
   bytes), `rustc_vv` (exact installed compiler output), `target`,
   `profile_features`, and booleans `capacity_accepted`, `local_cache_only`,
   `exact_checkout`, all true. The last three attest **completed operator review**,
   not runtime measurements inferred by the launcher. Pin only a reviewed
   default-branch workflow that enforces exact source/toolchain and local-cache
   rules above; the historical workflow in S2 does not satisfy that contract.
   A changed compiler/build configuration creates a cold key; changed workflow
   bytes require fresh policy review. Install cache root root:root 0700 and jobs
   root root:root 0755. Keep enough disk for concurrent private copies, source,
   incoming and current generations. Preparation retains published compatibility keys, including on cold-key TRAIN
   and failed MAIN runs. Only successful MAIN publication atomically replaces
   its manifest, then prunes to one published key and generation per slot/shard
   under the slot lock. Stop loops for manual quarantine or crash recovery.
   The launcher creates and validates both root-owned slot parents before either
   unit starts, so mandatory sibling hiding also works on a fresh installation.
   Traversal parents use explicit 0755 modes and `current` uses 0711 even under
   operator umask 077; work and cache trees remain private.
7. Review exact sudoers arguments with `visudo`; only the operator gets:

   ```sudoers
   dirtlab ALL=(root) NOPASSWD: /usr/local/sbin/aida-ci-one-job s0, /usr/local/sbin/aida-ci-one-job s1
   ```

   No empty-argument/legacy launcher grant, arbitrary paths or extra arguments.
   Verify `sudo -k; sudo -n -l` and that all four runner accounts lack sudo.
8. Start separate foreground operator sessions running installed/reviewed
   `rearm.sh s0` and `rearm.sh s1` only for controlled acceptance. No unattended
   credential service is provided. Duplicate manually started loops and stale
   reservations refuse safely. An API/hook/sudo/runner error stops supervision.

Shared slice: CPUs 0–7, quota 800%, MemoryHigh 20G, MemoryMax 24G, swap 0.
Each slot: quota 400%, equal CPUWeight 100, MemoryHigh 10G, MemoryMax 12G,
swap 0, CARGO_BUILD_JOBS=4, RUST_TEST_THREADS=4. The other twelve dell1 cores
remain outside CI's cpuset. No third slot is supported. Preserve PrivateIPC,
PrivateTmp/devices, NoNewPrivileges, RestrictSUIDSGID, read-only host system,
ProtectHome and blocked host-control sockets. Do **not** add ProtectProc or
ProtectKernelTunables: their proc overmounts break nested bwrap tests. Never
unload host AppArmor; obtain narrow reviewed policy or use disposable VMs.

## Acceptance, recovery and rollback

S2 tests use fake forge/systemd plus real filesystem/socket operations; they do
not establish host isolation or measured warm-build equivalence. S3 must retain
positive private-cache writes plus direct/alias/symlink seed and sibling denial
attempts, cross-uid file/signal/proc denials, cache-miss clean-build equivalence,
train mutation with unchanged trusted hashes, failed-main preservation and
successful-main publication. Exercise ordinary/fork PRs, PR-edited YAML,
forged TRAIN labels, every mismatched/missing host-record field and merge groups.
Run all seven S1 regressions under the actual unit with executed test counts,
positive controls, and `AIDA_REQUIRE_BWRAP_LIVE=1`; no skips count as acceptance.
Run all four heavy shards through the two slots. Confirm no OOM/memory limit
failures, cgroup survivors, cleanup overlaps or namespace-policy regressions.

Only S3 changes workflow routing: TRAIN/MAIN labels for eligible heavy jobs,
hosted ordinary/fork PRs and merge groups; keep the seven lanes, timeouts and
hosted required aggregate. Then activate for a bounded window and compare at
least ten comparable green trains with matched hosted runs. Record run start
through aggregate finish, shard created/started/finished, queue median/p95,
slot/class, seed SHA/hit/age, cold/warm state, libtest thread count, memory peaks,
concurrent Codex lane count and cold-build load; include other hosted critical
paths. The 20–30 minute warm end-to-end target is a forecast, not S2 evidence.

On any trust/containment failure or skip, OOM, cleanup overlap, repeated rearm
failure, median duration/queue regression, or p95 worsening >20%: unset routing,
cancel queued self-hosted runs and start fresh hosted runs (reruns may retain
old routing). Stop both operator loops, stop/reconcile each service, capture
unit logs/properties and cgroup events, quarantine caches and retain diagnostics.
There is no automatic hosted failover. Never remove a job directory or reservation
until the slot lock is available and its cgroup is empty. After reconciling,
remove only that slot's `/run/aida-ci-sN.json`, stale gate socket and disposable
`current`; remove orphan GitHub registrations before restarting. A refused stop
retains the state intentionally. A crash during publication may leave an
unreferenced generation; the manifest remains authoritative. Root-only offline
pruning must retain the manifest's generation and validate paths before deletion.

The slot lock covers all cache reads and writes. A manifest swap removes superseded generations; preparation preserves all published compatibility keys, including on TRAIN cold misses and failed MAIN runs. Only successful MAIN publication removes stale key directories, retaining one published key and generation per slot/shard. The controller creates and validates both stable root-owned slot parents before either unit starts, and enforces traversal modes independently of the operator umask (jobs/slot/runtime parents 0755, current 0711; private work/cache trees remain 0700). No workflow routing is enabled by this change.
