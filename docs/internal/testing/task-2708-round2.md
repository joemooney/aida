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
  are conservatively refused. Sends are bounded to 64 iovecs, 1 MiB payload,
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
