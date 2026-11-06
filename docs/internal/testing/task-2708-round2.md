# Shared containment review, round 2

<!-- trace:TASK-2708 | ai:codex -->

The binding 2026-11-06 advisor rulings govern this revision of PR #3449.

* F1/F2: job 127746720835's stderr shows `unshare(CLONE_NEWUSER|CLONE_NEWNET)`
  returning EPERM, before socket inspection or loopback setup. The outer
  namespace check alone is insufficient. A disposable under-filter probe now
  reports `NESTED_USERNS_UNAVAILABLE`, and required-live never skips it. Claude
  preparation requests this probe before its seat preflight. The disposable CI
  VM unloads its explicit `bwrap-userns-restrict` profile as well as setting the
  existing userns sysctl. This is CI host preparation, not a runtime fallback.
  The [upstream AppArmor profile](https://gitlab.com/apparmor/apparmor/-/raw/master/profiles/apparmor/profiles/extras/bwrap-userns-restrict)
  transitions children to `unpriv_bwrap`, which denies capabilities even when
  the global userns sysctl is zero. CI logs establish EPERM; attributing it to
  that profile is an inference to be confirmed by the next runner execution.
  Failed live scripts are read before asserting status, preserving their output.
* F3: a foreign socket must have a kernel-proven owner-userns ancestry reaching
  the pinned lane userns. SIOCGSKNS, NS_GET_USERNS and NS_GET_PARENT failures
  refuse. The broker connects the pinned socket, never CONTINUE. Tests cover
  lane, descendant, host and foreign evidence plus a relay reconnect.
* F4: sendto/sendmsg are notifications. Named AF_UNIX destinations refuse;
  arguments are copied and executed on the pinned socket, never checked then
  continued against mutable memory. Anonymous connected IPC and SCM_RIGHTS
  remain supported. Named sendmsg destinations of every family and sendmmsg
  return EACCES, including connected UDP sendmmsg and named UDP sendmsg to a
  reachable lane/descendant resolver. These are AC1 compatibility deviations
  requiring advisor acceptance (Round 8 F2 below), not just unreachable-network
  refusals. Ancillary messages other than SOL_SOCKET/SCM_RIGHTS
  (including explicit SCM_CREDENTIALS) now refuse with EACCES. An SO_PASSCRED
  receiver sees the host broker's credentials rather than the trapped sender's
  (the broker PID appears as 0 in the lane PID namespace). These additional
  behavior deviations require advisor acceptance alongside Round 5 F2; this
  implementer does not claim that acceptance. Sends are bounded to 64 iovecs, 1 MiB payload,
  4 KiB ancillary data, and use MSG_DONTWAIT so they cannot block the broker.
  A live Unix datagram test checks delivery refusal and connected positive controls.
* F5: both host `.claude.json` and `.claude.json.backup` receive empty read-only
  file masks; directories receive empty read-only directory masks.
* F6/F9: expiry is captured from the validated readable descriptor actually
  used by the read-only bind. Host path replacement cannot change exit
  classification. Unreadable/invalid credentials have distinct named errors.
  No evidence supports `claude auth status` as a refresh operation, so it is
  removed. `CLAUDE_EXPIRED` fails fast and directs the operator to `claude /login`.
  This decision is also recorded on the spec.
* F7: `task_2708_claude_prepare_fake_home` drives actual preparation with a
  synthetic trusted repository, executable and credentials. It checks masks,
  environment rows, updater disabling, read-only descriptor binding, symlink,
  hardlink, invalid/expired credentials, and mismatching credential-owner UID
  through preparation. The owner mismatch is synthetic (no host chown required).
  Where bwrap is available, the prepared mount vector proves masking and denied
  credential writes. It does not manufacture a production authority receipt.
* F8: at most 16 nested connects may be admitted. Each runs in an isolated
  interpreter with a three-second parent-enforced deadline. A malicious sibling
  changing shared socket flags or SO_SNDTIMEO cannot remove the deadline.
  Saturation returns EAGAIN; timeout returns ETIMEDOUT. Synthetic tests exercise
  saturation, capacity restoration and a deliberately wedged connect.

The mechanical CI merge-resolution commit remains in branch history. Native
implementer seats leave code uncommitted for host publication, and do not clear
merge holds or record review approval.

## Validation

Validated locally on 2026-11-06:

* `AIDA_REQUIRE_BWRAP_LIVE=1 AIDA_REQUIRE_CODEX_CONTAINMENT_LIVE=1 cargo test -p aida-cli-lib --lib --profile agent task_2708_ -- --nocapture`: 29 passed, including every live network regression and fake-HOME preparation.
* The same compiled test binary, with both required-live flags, filtered to `task_2707_`: 21 passed. The installed-Codex native-composition arm remains separately opt-in and was not requested or exercised.
* `make check-ci-fast`: all 38 fast gates passed.
* A fresh `cargo check -p aida-cli --no-default-features` against the final source passed after the in-flight fast gate's earlier check.
* `cargo fmt --all -- --check`, `git diff --check`, Python source compilation and CI YAML parsing passed.
* `python3 -m unittest tests/test_ci_test_shards.py`: 25 passed.
* The exact CI under-cap-drop nested namespace probe passed locally.

The hosted runner must still confirm the CI-only AppArmor preparation. No hosted
CI run, publication, review approval, merge-hold clearance or queue completion was
performed from this native implementer seat. All six changed files are left
uncommitted for the host launcher. The mechanical bdcec264ee88 merge-resolution
commit is still an ancestor of HEAD.

## Round 3 review

<!-- trace:TASK-2708 | ai:codex -->

This revision addresses all four findings against PR #3449 at df6bf0e76661.
The 2026-11-06 binding advisor rulings remain in effect.

* F1: blocking sends run outside the broker event loop, using MSG_DONTWAIT
  attempts and POLLOUT waits with a three-second monotonic deadline. At most
  16 send workers retain copied payloads and duplicated descriptors. A full
  blocking socket retries EAGAIN until progress or ETIMEDOUT; resource
  saturation returns ENOBUFS. O_NONBLOCK and per-call MSG_DONTWAIT still return
  EAGAIN immediately. No shared descriptor flags or timeouts are changed.
* F2: stream snapshots copy the first 1 MiB and at most 64 iovecs, then return
  the actual partial count. Datagram and seqpacket messages retain their
  boundaries: excessive payload or iovec counts refuse with EMSGSIZE.
* F3: named non-Unix sends call the same socket_scope proof as connects.
  Only lane and proven descendant ownership pass; foreign namespaces and
  lookup errors refuse. Named Unix sends and named sendmsg remain refused.
* F4: nested_userns is a field on each CONTAINED_VENDORS row. Shared preparation
  serializes that field. Vendor-row and fake-HOME prepare tests assert it.

The new live stream regression fills a blocking socket, delays the reader,
then verifies a successful send, a complete 4 MiB sendall and sendmsg progress
across 100 iovecs. The live UDP regression passes an actual host-netns socket
through permitted private Unix IPC with SCM_RIGHTS; its named send must refuse,
and the host listener must receive no bypass datagram. The host sender has a
successful pre-lane positive control.

The synthetic broker regression uses real sockets and process memory, replacing
only notification and pidfd transport. It covers actual host UDP refusal,
foreign/unreadable proof, lane/nested positive controls, oversized datagram
refusal for scalar payloads, oversized vectors and vector payloads, nonblocking
flags, worker timeout, capacity restoration and saturation.

### Round 3 validation

* `make check-ci-fast`: all 38 fast gates passed, including the
  feature-disabled CLI check and TUI seam check. Optional ref-refresh attempts
  reported read-only Git metadata; the gates completed successfully.
* Direct execution of the embedded synthetic send regression passed.
* Python source compilation, `cargo fmt --all -- --check` and
  `git diff --check` passed.
* Initial targeted `cargo test -p aida-cli-lib --lib --profile agent task_2708_`:
  32 passed, including all new live regressions (no live skips).
* The compiled targeted binary with both required-live flags, filtered to
  `task_2707_`: 21 passed. Its separately opt-in installed-Codex native
  composition arm was not exercised.
* Final-source `AIDA_REQUIRE_BWRAP_LIVE=1
  AIDA_REQUIRE_CODEX_CONTAINMENT_LIVE=1 cargo test -p aida-cli-lib --lib
  --profile agent task_2708_ -- --nocapture`: 32 passed in 2.18 seconds,
  with no live skips. This includes blocking backpressure, the complete 4 MiB
  upload, large-iovec progress, passed host-UDP refusal, and final synthetic
  scalar/vector bounds and worker checks.
* Final documentation inventory/link/path checks passed.

The native implementer leaves changes uncommitted for host publication. No
review verdict, merge-hold clearance, ship, or queue completion is attempted.
The mechanical bdcec264 CI merge-resolution commit remains an ancestor of HEAD.

## Round 4 review

<!-- trace:TASK-2708 | ai:codex -->

This revision addresses all four findings against PR #3449 at 9e1319e8b3fb.
The round 3 deadline and saturation behavior above is historical and superseded.
The mechanical bdcec264ee88 CI merge-resolution commit remains an ancestor.

* F1: blocking sends have no broker-imposed deadline. Workers try MSG_DONTWAIT,
  poll POLLOUT in at most 50 ms steps and recheck NOTIF_ID_VALID each time.
  SO_SNDTIMEO is captured from the pinned socket at admission; zero means no
  deadline, and expiry returns EAGAIN. At most 16 workers snapshot arguments.
  Saturated calls queue their notification and pinned socket without copying
  payloads or rights. Queue waits count toward the socket timeout; cancelled
  queued calls release their socket. The broker continues servicing other
  notifications while sends wait. No shared descriptor flags/timeouts change.
* F2: the lane installs NEW_LISTENER | WAIT_KILLABLE_RECV. Kernels refusing
  these flags fail with SECCOMP_WAIT_KILLABLE_RECV_UNAVAILABLE, without fallback.
  Handled signals cannot restart a notification after its send side effect.
* F3: host send attempts use MSG_NOSIGNAL, then EPIPE explicitly delivers
  SIGPIPE with tgkill to the notification's host TID unless the original call
  requested MSG_NOSIGNAL. Delivery failure reports SEND_SIGPIPE_FAILED and EIO.
  Nonleader fd duplication uses PIDFD_THREAD on Linux 6.9+. The round 4
  implementation intended to prove KCMP_FILES equality before using the leader's
  pidfd on older kernels, but its ENOENT/ESRCH fallback was unreachable: plain
  pidfd_open(nonleader) returns EINVAL. Its synthetic test modeled the wrong
  errno; round 4 did not establish Linux 5.19-6.8 nonleader compatibility. Round 5
  below corrects both the code and evidence. Thread-directed SIGPIPE delivery
  does not grant foreign socket scope.
* F4: the round-specific AGENTS.md paragraph is removed. OVERVIEW.md retains
  only a short architecture-level sentence. Detailed evidence stays here and
  in the owning spec comment; the host launcher supplies the PR publication.

The live blocking regression stalls its reader for 3.5 seconds, verifies the
marker, complete 4 MiB upload and 100-iovec sendmsg, checks a 500 ms socket
send timeout, then releases 20 blocked senders through the 16-worker limit.
A separate live regression fires a 1 ms SIGALRM interval throughout scalar and
vector sends and compares every received byte; it also checks SIGPIPE handlers,
MSG_NOSIGNAL suppression, a blocked nonleader's sigtimedwait, and a child with
SIG_DFL. Synthetic coverage includes queued timeout accounting, worker and queue
cancellation cleanup, old-kernel named filter refusal, and proven/unproven
shared-file-table fallback.

### Round 4 validation

Validated locally on 2026-11-06 against the final source:

* `AIDA_REQUIRE_BWRAP_LIVE=1 AIDA_REQUIRE_CODEX_CONTAINMENT_LIVE=1 cargo test
  -p aida-cli-lib --lib --profile agent task_2708_ -- --nocapture`: 33 passed,
  no failures or live skips. A second final-source invocation also passed.
* The same compiled test binary with both required-live flags, filtered to
  `task_2707_`: 21 passed. Installed-Codex native composition remains separately
  opt-in and was not exercised.
* `make check-ci-fast`: all 38 fast gates passed, including the feature-disabled
  CLI check. Optional ref refreshes reported the seat's read-only Git metadata;
  the gates completed successfully.
* Direct real-filter signal-storm/SIGPIPE regression: five repetitions passed.
  Direct blocking/backpressure/20-sender regression and final synthetic broker
  regression passed as well.
* Python source compilation, `cargo fmt --all -- --check`, and
  `git diff --check` passed.

All four round 4 findings are addressed; none are deferred or disputed. The
native seat leaves the six edited files uncommitted for host publication and
records the result on TASK-2708. Store comment persistence queues its Git commit
for the host. The existing merge hold remains; this seat performs no merge-hold
clearance, review verdict, source commit/push, or queue completion.

## Round 5 review

<!-- trace:TASK-2708 | ai:codex -->

This revision addresses the older-kernel fallback finding against PR #3449 at
b29dacfa45b4. The mechanical bdcec264ee88 CI merge remains in branch history.

* F1: when PIDFD_THREAD returns EINVAL, target_socket reads the trapped TID's
  Tgid first. Leaders use their own plain pidfd; nonleaders go directly through
  kcmp(KCMP_FILES) and open only the proven leader's pidfd. A different table or
  failed proof refuses with SOCKET_THREAD_FILE_TABLE_UNPROVEN. Other
  PIDFD_THREAD errors still refuse without falling back. The new standalone
  regression uses a real nonleader TID and a kernel shim returning EINVAL for
  both unsupported PIDFD_THREAD and plain nonleader opens. It checks leader
  handling, nonleader proof-before-duplication ordering, different/unreadable
  file-table refusals and permission refusals. The shim's nonleader plain open
  must never be invoked by the fixed path. This replaces the inaccurate
  ENOENT-based test; local live tests on a newer kernel alone do not prove
  Linux 5.19-6.8 compatibility.
* F2: **advisor acceptance pending**. All contained vendors, including Codex,
  require Linux 5.19+ for NEW_LISTENER | WAIT_KILLABLE_RECV. Once the broker
  receives a notification, handled signals (including SIGINT, SIGALRM, handled
  SIGTERM and Go SIGURG) are deferred until it responds. A blocking send without
  SO_SNDTIMEO may wait indefinitely for peer progress, so a handled signal cannot
  interrupt that wait as an ordinary send can. Fatal signals still terminate;
  notification cancellation releases broker resources. SO_SNDTIMEO still
  bounds configured socket waits. This is a keep-existing-behavior deviation
  across every lane, retained to prevent the send-side-effect/restart race and
  duplicate stream bytes. Restoring ordinary signal interruption requires a
  reviewed alternative that preserves exactly-once sending; simply dropping
  WAIT_KILLABLE_RECV would reintroduce the round 4 correctness defect. Neither
  the prior instruction to add the flag nor the review's description of this as
  an "accepted price" is recorded here as advisor signoff. A named F2 punt asks
  the advisor to accept these two deviations on TASK-2708 or direct a redesign.
  No acceptance is claimed by this implementer.
* Ancillary credentials (Round 7 F3): **advisor acceptance pending** alongside
  F2. The broker accepts only SOL_SOCKET/SCM_RIGHTS control messages; explicit
  SCM_CREDENTIALS and other ancillary types return EACCES. For implicit
  SO_PASSCRED delivery the receiver sees the broker's credentials, including PID
  0 when viewed from the lane PID namespace, rather than the trapped task's PID.
  These are additional AC1 compatibility deviations, not evidence of sender
  impersonation support. The Round 7 punt requests an explicit acceptance or a
  direction to implement sender-credential forwarding with regression coverage.
* UDP API compatibility (Round 8 F2): **advisor acceptance pending** alongside
  F2 and ancillary credentials. The filter refuses every sendmmsg with EACCES,
  including unnamed messages on connected UDP sockets. The broker also refuses
  named sendmsg for every address family, even when socket_scope proves lane or
  descendant ownership. This can break glibc parallel A/AAAA resolution using
  connected sendmmsg, plus resolvers, QUIC and mDNS callers using named sendmsg,
  even with a reachable resolver in a nested network namespace. The Round 8
  request asks the advisor to accept both API restrictions explicitly or direct
  sendmmsg brokering and scope-proven named network sendmsg support. Named
  AF_UNIX sends must still refuse under the binding F4 ruling. Refusal tests
  pin EACCES; they do not establish compatibility or advisor acceptance.


### Round 5 validation and handoff

Validated locally on 2026-11-06:

* `AIDA_REQUIRE_BWRAP_LIVE=1 AIDA_REQUIRE_CODEX_CONTAINMENT_LIVE=1 cargo test
  -p aida-cli-lib --lib --profile agent task_2708_ -- --nocapture`: 34 passed,
  no failures, ignored tests or live skips. The host kernel is 7.0.0-42-generic;
  older-kernel behavior is covered by the explicit errno shim, not an actual
  Linux 5.19-6.8 boot.
* The same compiled binary's `task_2707_` filter with both required-live flags:
  21 passed. Installed-Codex native composition remains separately opt-in and
  was not exercised.
* The standalone old-kernel regression passes with the edited broker and fails
  against the previous HEAD broker with the modeled plain-nonleader EINVAL.
  The direct snapshot/timeout/queue/cancellation regression also passes.
* `make check-ci-fast`: all 38 fast gates passed, including feature-disabled
  CLI checking, the TUI seam and formatting. Optional ref-refresh attempts
  reported read-only Git metadata; their guards passed.
* Python source compilation and `git diff --check` passed.

F1 is fixed. F2 remains pending explicit advisor signoff for both deviations,
with a named punt on TASK-2708; it is not disputed or declared accepted.
The three changed files are contained_host.py, containment/tests/launch_tests.rs
and this testing note. Edits remain uncommitted for the native-seat host handoff.
The punt and existing supervised merge hold remain publication vetoes; this
seat performs no commit/push, review recording, hold clearance or queue completion.


## Round 7 review

<!-- trace:TASK-2708 | ai:codex -->

Reviewed base: `aede2be58b3e9d17bd61db0d532f6eef3669b8f5`, PR #3449.
The mechanical `bdcec264ee88` merge remains an ancestor of HEAD.

* F1: no explicit advisor acceptance of Round 5 F2 is present in TASK-2708
  comments 1–58 or in the supplied ruling. The Linux 5.19+ floor and deferred
  handled signals remain acceptance pending above. Removing WAIT_KILLABLE_RECV
  would restore the duplicate-send race; no implementer acceptance or redesign
  is substituted for the requested advisor decision.
* F2: the earlier `2d054799c40715d859ed7c1295c7c4b4d9cfdbf8` failure is now
  diagnosed from [run 42216248331](https://github.com/joemooney/aida/actions/runs/42216248331).
  All six test shards passed. The gates job passed its required-live bwrap step,
  then failed the help-golden co-change guard because it could not resolve a
  merge base with `origin/main` after deepening the shallow checkout by 200.
  The aggregate Build failure followed that gates failure. This was not a
  containment test failure; exact-head CI must still establish non-recurrence.
* F3: the earlier summary and the section next to Round 5 F2 now explicitly
  describe EACCES for ancillary types other than SOL_SOCKET/SCM_RIGHTS,
  including SCM_CREDENTIALS, and broker identity for SO_PASSCRED receivers.
  Acceptance is pending, and the named punt includes this compatibility delta.
* F4: the required-live fake-HOME result check now emits an explicit GitHub
  `::error::` annotation naming `task_2708_claude_prepare_fake_home` before
  exiting nonzero when that test does not report ok.

### Round 7 CI evidence at the reviewed base

At `aede2be58b3e`, the [gates job](https://github.com/joemooney/aida/actions/runs/42220164171/job/127899913579)
passed, including the required-live bwrap gate (`9 of 9` TASK-2708 live tests
reported ok) and the help-golden guard that failed at `2d054799`. The uploaded
`test-timings-lib-1` JUnit artifact records
`launch::contained::tests::task_2708_target_socket_old_kernel_thread_file_tables`
as a passing testcase. All seven shards and the required `Build (ubuntu-latest)` passed by
2026-11-06 13:10 UTC; only the supervised merge-hold gate failed.
These are reviewed-base results; this round's edits still require host
publication and CI at the resulting head. The supervised merge hold remains.

### Round 7 local validation

* `make check-ci-fast`: all 38 gates passed, including feature-disabled CLI,
  TUI seam, formatting, documentation guards and trace checks.
* `python3 -m unittest tests/test_ci_test_shards.py`: 25 passed.
* Executed the edited fake-HOME check from the parsed workflow with an empty
  result file and a passing row: absence exited 1 with the named annotation;
  the passing row exited 0.
* CI YAML parsing and `git diff --check` passed.
* `AIDA_REQUIRE_BWRAP_LIVE=1 AIDA_REQUIRE_CODEX_CONTAINMENT_LIVE=1 cargo test
  -p aida-cli-lib --lib --profile agent task_2708_ -- --nocapture`: 34 passed,
  no failures, ignored tests or live skips. This includes fake-HOME preparation,
  the old-kernel thread-file-table shim and all nine live regressions.

Round 7 F1 and the acceptance part of F3 are explicitly punted on TASK-2708
as a design fork; neither finding is disputed. No acceptance is claimed.
The two edited files are `.github/workflows/ci.yml` and this testing note.
Native-seat edits remain uncommitted; the host must honor the punt and existing
merge hold. No source commit/push, review approval, hold clearance or queue
completion was performed. Publication-head CI remains required after any
future host publication.

## Round 8 review

<!-- trace:TASK-2708 | ai:codex -->

This revision addresses the findings against `e64a75816810`. The seat starts at
`75eb8ca95841`, which includes the host's main merge. The mechanical
`bdcec264ee88` CI merge remains an ancestor. No production containment code is
changed in this round.

* F1: the three compatibility decisions remain **advisor acceptance pending**:
  handled-signal deferral (possibly indefinite without SO_SNDTIMEO), Linux 5.19+
  for every lane, and ancillary credential rejection/broker identity. The
  current spec comments include the Round 7 punt and Round 8 review/rework,
  with no new acceptance ruling. Round 5 F2 and Round 7 F3 above therefore
  retain their pending labels. The new punt explicitly requests acceptance or
  an exactly-once/credential-forwarding redesign; this finding is not disputed.
* F2: take the review's acceptance-request option. The Round 5 list now also
  names connected UDP sendmmsg and named network sendmsg, their EACCES results,
  and their resolver/QUIC/mDNS impact. The existing nested-netns live regression
  now uses reachable UDP sockets: connected send and unnamed sendmsg succeed,
  a two-message connected sendmmsg returns EACCES without delivery, and named
  sendto succeeds while named sendmsg returns EACCES on both connected and
  unconnected sockets. The filter regression pins the exact seccomp EACCES
  encoding. The synthetic broker regression pins named sendmsg EACCES with
  lane/nested as well as foreign/unreadable scope evidence. These extend
  existing tests, preserving the required-live count of nine. Acceptance of
  both API restrictions is still requested, not assumed.
* F3: no green-head claim. At inspection, reviewed head `e64a75816810` had
  successful cli-0, cli-1, contracts and lib-1 checks, with lib-0, rest and gates
  still running. The current PR head `75eb8ca95841` had contracts running and
  the other six shards queued. Both heads had the expected supervised hold
  failure. See the [reviewed-head CI run](https://github.com/joemooney/aida/actions/runs/42223458416)
  and [current-head CI run](https://github.com/joemooney/aida/actions/runs/42224903365).
  Neither establishes the required all-shards/Build result for a future
  publication head. The next reviewer must check the required-live count,
  fake-HOME result/diagnostic and old-kernel testcase evidence there.
* F4: AC1 remains partial. Only two headless paths use the opt-in shared Claude
  wrapper; interactive, background, coordinator and resume launches still
  refuse under that opt-in. Advisor confirmation of the rollout gate and an
  interactive/background follow-up slice remain required before AC2 evidence
  work. This implementer does not claim that rollout decision or completion.

### Round 8 validation and handoff

* Direct execution of the edited embedded filter and synthetic broker Python
  regressions passed (52 filter decisions; send bounds/namespace/queue suite).
* An unfiltered positive control using the live regression's ctypes structures
  delivered both sendmmsg datagrams, validating the two-message fixture layout.
* All three edited embedded Python scripts compile; the live test count is nine.
* `cargo fmt --all -- --check` and `git diff --check` passed.
* `make check-ci-fast`: all 38 gates passed, including feature-disabled CLI,
  TUI seam, containment conformance fixtures, formatting and documentation
  checks. Optional Git ref refreshes reported read-only metadata; the gates
  completed successfully.
* Targeted `aida lane job run --wait 1 -- cargo test --profile agent -p
  aida-cli-lib --lib task_2708_ -- --nocapture` was refused: lease
  `01a1b105b8bf` has no lane job runner. No alternative Cargo test launch was
  used. **untested=true** for the Rust harness and edited live assertions;
  the two direct synthetic executions are not live-boundary evidence.

The native seat leaves the two changed files uncommitted. The design-fork punt
and supervised hold remain publication/merge vetoes. No source commit/push,
review verdict, hold clearance or queue completion is performed.
