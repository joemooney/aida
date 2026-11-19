Run `aida doctor --lane-host` on the target machine. Apply safe user-level permission and Cargo configuration fixes with `aida doctor --lane-host --fix`. See docs/cli/07-project-setup.md#lane-host-readiness for the checks, advisory notes and fix scope. Codex uses no extra writable roots (`writable_roots=[]`); checks and fixes honor CODEX_HOME.

## Remaining manual setup
- Configure SSH access and any required host/DNS pin. For the reference GitLab remote ssh://git@gitlab.joemooney.com:2222/ai/aida.git, LAN machines may need an /etc/hosts pin to bypass Cloudflare Access: 192.168.179.100 on other hosts, 127.0.0.1 on dell1.
- Clone the project (reference checkout: ~/ai/aida) and configure remote/mirror access.
- Install AIDA and vendor CLIs. Put ~/.local/bin on PATH in .profile and .bashrc; use a login shell for remote commands, for example `ssh dell1 'bash -lc "aida doctor --lane-host"'`.
- Log in with the selected vendor CLI, for example `codex login`. Credential-source presence does not verify token validity; unsupported auth probes remain advisory.
- Run any system-level install/userns/AppArmor commands printed by `aida doctor --fix-sandbox` yourself.
- Fetch dependencies on the host before starting lanes. The doctor requires a configured executable Cargo wrapper and runs a lane-job sandbox probe that starts a private sccache server, queries its transport, and writes/reads a temporary cache file. Sandbox, transport or write failures produce a required finding with a fix hint.

## Live seat-clone tests

The BUG-2415 explicit-refspec test uses a disposable local origin and a
fixture-provided mail-writer inventory. It requires bubblewrap and working
unprivileged user namespaces. Set `AIDA_REQUIRE_BWRAP_LIVE=1` to make a
missing sandbox a test failure.

The TASK-2712 test also needs `codex` on PATH. It first checks that
`codex sandbox -P :workspace` can write to an extra root outside its working
directory with a temporary Codex home. It also probes an ungranted sibling
directory. The test prints a named skip if Codex denies the extra-root write,
still allows writes to the ungranted sibling in a `/tmp` fixture despite the
temp exclusions, or reports an app-server socket directory ownership/mode
error. Run the live check from a host checkout outside `/tmp` with a Codex
build that honors `sandbox_workspace_write.writable_roots` and a user-owned app-server
socket directory with mode `0700`. An outer Codex sandbox can mask that
directory. For a required-live run, set `AIDA_REQUIRE_CODEX_SANDBOX_LIVE=1`
and point `AIDA_TEST_AIDA_BIN` at the built AIDA executable. Required-live
runs fail on those preflight errors. Once the preflight passes, the test
requires the production launch arguments, cache and store writes, and
protected-path denials to pass. Both fixtures support unseeded Cargo targets.
<!-- trace:BUG-2676 | ai:codex -->

## Lane wrapper publication

Use `bash scripts/lane-run.sh SPEC` from the primary checkout (additional
`aida queue work` flags pass through). Replace host-local `lane-run.sh` and
`auto-publish.sh` chains with this entry point. It delegates the entire lane
lifecycle to AIDA, including its clean-exit publication checks. It never follows
a punt, admission refusal or crash with a separate `ship --commit-pending`.
Direct host seat publication also refuses persisted host or seat punt/hold
signals before committing or pushing. A missing terminal outcome is not evidence
that an external wrapper may publish retained work.

Before starting the lane, the wrapper runs `aida dev check-fresh` and refuses
when the host's binary is more than 10 merged changes or 6 hours behind
`origin/main`. Rebuild with `aida dev build` from a clean `main`, or set
`AIDA_LANE_ALLOW_STALE=1` to launch anyway. A host launcher's own `--check`
mode can call `aida dev check-fresh --fetch` to report the same verdict before
it starts anything. A host autobuild that fast-forwards `main` must account
for a skip-worktree override of `.aida/config.toml`: `git merge --ff-only`
refuses whenever an incoming commit changes that file, so the binary silently
stops being promoted. After the fast-forward it must run `aida dev build`
inside that checkout, so the build is the checkout's own new `HEAD`; it never
builds an older ref or a different clone.
<!-- trace:TASK-3812 | ai:claude -->

Host rollout still requires locating and replacing each local wrapper; updating
this repository does not modify scripts in host home directories.
<!-- trace:BUG-2648 | ai:codex -->

## Scheduled autobuild

On each Linux lane host, use the operator-owned
[`scripts/aida-autobuild.sh`](../../scripts/aida-autobuild.sh) as the single
scheduled builder. It expects Bash, Git, GitHub CLI with read access to the
CI workflow, GNU `timeout`, `flock`, and Python 3.11 or later. Create the log
directory before scheduling. The defaults are
`AIDA_REPO_DIR=$HOME/ai/aida`, `AIDA_GH_REPO=joemooney/aida`, and
`AIDA_AUTOBUILD_LOG=$HOME/.aida/autobuild.log`. Cron, `aida dev build`,
and dev-autopromote take the same exclusive lock on the primary checkout's
`.git` directory.
Cron logs `build in progress, skipped` and exits before fetching if another
builder holds that lock.

After the change lands, the host operator installs the script to
`~/bin/aida-autobuild.sh` and uses the existing 30-minute schedule or adds one
if absent. Retire any other scheduled builder for this checkout. For a
checkout at `~/ai/aida`, install and schedule it with:

```bash
install -m 755 ~/ai/aida/scripts/aida-autobuild.sh ~/bin/aida-autobuild.sh
```

```cron
*/30 * * * * /home/dirtlab/bin/aida-autobuild.sh
```

Run the installed script with `--dry-run` first. It fetches and checks the CI
state and config merge without changing the checkout or building. Run a normal
tick on a green `origin/main` and confirm the log contains
`promoted and verified: <SHA>` when a build was needed. The script verifies the
launcher PATH's binary SHA and runs `~/lane-start.sh --check` before recording
that line. If the binary was already current, check `aida --version` and
`~/lane-start.sh --check` directly for rollout evidence. Confirm the next
scheduled tick runs under the same user and environment.

The tick waits up to about 25 minutes for CI on its selected SHA. After green
CI it fetches again and defers if the remote tip moved. A conflicting config
override skips the tick before changing the checkout. If interrupted while
fast-forwarding, inspect the recovery directory logged before the change. It
contains the original override and the proposed rebased file. Reconcile that
copy against the checkout's current `HEAD` before marking the config
skip-worktree again. Avoid editing that override or advancing the primary
checkout manually while a scheduled tick runs.

## Verify live binaries from the operator machine

Fetch `origin/main` in the reporting checkout, then supply each lane host's
SSH alias with a trusted host key and non-interactive authentication. The
report runs the same PATH-selected `aida --version` used by the launcher. It
compares every response with the reporting checkout's pinned `origin/main` and
shows the binary SHA, build age, and distance in first-parent changes. Unreachable hosts and unresolved SHAs produce individual `unknown`
rows. Add `--json` for machine-readable output.

```bash
git fetch origin main
aida doctor --lane-binaries dell1 --lane-binaries second-lane-host
```
<!-- trace:TASK-3812 | ai:codex -->
