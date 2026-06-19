<!-- trace:TASK-867 -->

# The bubblewrap OS sandbox (`os_wrap`)

AIDA can wrap the agent process it launches in an OS-level sandbox built on
[bubblewrap](https://github.com/containers/bubblewrap) (`bwrap`). This document
describes **AIDA's own `os_wrap` mechanism** — what it binds, how it fails
closed, the host it needs, and exactly which paths it covers today. It is *not*
about Claude Code's own internal bubblewrap sandbox (that is configured
separately via `[contained] enable`, the Claude-Code-native `--settings`
posture — see [`../cli/03-work-autonomy.md`](../cli/03-work-autonomy.md)).

## TL;DR

- **Default OFF.** The OS boundary is strictly opt-in via `[contained] os_wrap`.
- **Write-confinement.** When on, the agent runs under `bwrap` with the host
  filesystem mounted **read-only**, and read-write access bound to only the
  worktree, the requirement store, and the build/auth caches.
- **Headless drains only, today.** It wraps the unattended `claude -p` drain
  paths. The interactive `aida agent new` launch is **not** yet wrapped
  (TASK-864).
- **Fail-closed.** If `bwrap` is missing or the kernel blocks unprivileged user
  namespaces, the launch **errors** rather than running the agent unconfined.
- **Discovery:** `aida doctor` reports availability; `aida config show` renders
  the resolved posture.

## What `os_wrap = true` actually does

With the knob on, instead of spawning the headless agent as `claude …`, AIDA
spawns it as:

```
bwrap <confinement-flags…> claude <args…>
```

The confinement flags establish a write-confined namespace:

| Mount | Mode | Why |
| --- | --- | --- |
| `--ro-bind / /` | read-only | The whole host filesystem is readable but **cannot be written** outside the explicit read-write set below. (Replaced by an enumerated set when `read_allowlist` is used — see below.) |
| the code worktree | read-write | The drain runs *in* it; the one always-present writable surface. |
| `.aida-store` (sibling of the worktree) | read-write | The requirement store the drain updates. |
| `$CARGO_HOME` (or `~/.cargo`), `~/.npm` | read-write | Cargo/npm registry + build caches must stay writable or builds fail mid-run. |
| `~/.claude`, `~/.claude.json` | read-write | Claude Code's session state + auth, which it writes during a run. |
| fresh `/dev`, `/proc`, `tmpfs` `/tmp` | — | A sane process environment for the toolchain. |
| `--die-with-parent` | — | A killed drain tears its sandbox down — nothing is left behind. |

The read-write caches use a *try*-bind: a path that doesn't exist on a fresh
machine is skipped rather than aborting the launch. The worktree itself is a
hard bind (it always exists).

**Network is shared.** `os_wrap` is a *filesystem write-confinement* boundary,
not a network jail. It never passes `--unshare-net`. Egress policy is a separate
concern, governed by `[contained] allowed_hosts` and
`[contained] managed_domains_only` (documented in
[`../cli/03-work-autonomy.md`](../cli/03-work-autonomy.md)).

## Fail-closed behavior

`os_wrap` never silently degrades to running the agent unconfined. Before
wrapping a drain it checks two things and **errors** (with remediation) on
either failure:

1. **`bwrap` on `PATH`.** If bubblewrap isn't installed, the launch refuses:
   *"`[contained] os_wrap` is enabled but `bwrap` (bubblewrap) was not found on
   PATH — install bubblewrap or unset os_wrap."*

2. **A live userns self-test.** `bwrap` can be installed yet unable to create an
   unprivileged user namespace. A trivial `bwrap … true` preflight catches that
   *before* the real drain, so you get an actionable message instead of a
   cryptic `uid map: Permission denied` mid-run.

## Host requirement: unprivileged user namespaces

`bwrap` needs the kernel to permit **unprivileged user namespaces**. On recent
Ubuntu (23.10+/24.04) AppArmor restricts this by default, so even with `bwrap`
installed the self-test fails and `os_wrap` fails-closed. There are **two ways**
to grant it, and they are *not* equivalent in blast radius:

- **A per-binary AppArmor profile** that allows only `bwrap` to create user
  namespaces — **recommended on any managed or shared host.** The host-wide
  restriction stays on for everything else.
- **The host-wide sysctl** `kernel.apparmor_restrict_unprivileged_userns=0` —
  quick, fine on a personal dev box, but it **turns the restriction off for the
  whole machine** and is the change a security-conscious IT team will object to.

Both are detailed in [Permitting unprivileged user
namespaces](#permitting-unprivileged-user-namespaces-the-apparmor-profile-vs-the-sysctl)
below — including the profile to install and *why it is the defensible ask in a
controlled environment*.

Until userns is permitted, enabling `os_wrap` will (correctly) refuse to launch
drains rather than run them unconfined.

## Permitting unprivileged user namespaces: the AppArmor profile vs the sysctl

`bwrap` confines the agent by putting it in a fresh **user namespace** (that is
how an unprivileged process drops privileges and remaps mounts). On Ubuntu
23.10+/24.04 the kernel ships with that capability **restricted by default**
(`kernel.apparmor_restrict_unprivileged_userns=1`) — a deliberate hardening,
because unprivileged user namespaces have historically been a rich source of
local-privilege-escalation (LPE) kernel CVEs. To run `os_wrap` you must re-grant
the capability. *How* you grant it is the whole question.

### Recommended: a per-binary AppArmor profile (managed / shared hosts)

Grant the `userns` capability to **only** the bubblewrap binary, leaving the
host-wide restriction on for every other process:

```
# /etc/apparmor.d/bwrap
# Allow ONLY bubblewrap to create unprivileged user namespaces. The host-wide
# restriction (kernel.apparmor_restrict_unprivileged_userns=1) stays ON; every
# other unprivileged process is still blocked.
abi <abi/4.0>,
include <tunables/global>

profile bwrap /usr/bin/bwrap flags=(unconfined) {
  userns,
  include if exists <local/bwrap>
}
```

Install and load it (it auto-loads on every boot thereafter):

```bash
# Confirm the path first — the profile's attachment path must match the binary:
which bwrap                                  # e.g. /usr/bin/bwrap

sudo tee /etc/apparmor.d/bwrap >/dev/null <<'EOF'
abi <abi/4.0>,
include <tunables/global>

profile bwrap /usr/bin/bwrap flags=(unconfined) {
  userns,
  include if exists <local/bwrap>
}
EOF

sudo apparmor_parser -r /etc/apparmor.d/bwrap   # load now; persists across reboots
```

Verify with the non-sudo self-test (`aida doctor --fix-sandbox`, or
`bwrap --ro-bind / / true` should now exit 0), and **leave**
`kernel.apparmor_restrict_unprivileged_userns=1`.

> **On `flags=(unconfined)`.** This profile does *not* otherwise confine
> `bwrap` — it exists solely to grant the `userns` capability. That is correct
> here: `bwrap` is itself the confinement mechanism (it builds the agent's
> sandbox), and wrapping it in a *restrictive* AppArmor profile tends to break
> the sandboxes it sets up. A site that wants AppArmor to additionally mediate
> `bwrap`'s own accesses needs a much larger, carefully-tested profile — out of
> scope here.

### Why the per-binary profile is acceptable in a reasonably well-controlled environment

The sysctl and the profile both let bubblewrap create a user namespace, but a
security review should *prefer* the profile, for six reasons:

1. **Least privilege / scoped exception.** The sysctl re-enables unprivileged
   user namespaces for *every* process run by *every* user on the host. The
   profile grants the capability to exactly one named binary at one path. Every
   other unprivileged process stays blocked — the LPE attack surface the Ubuntu
   restriction was added to close **remains closed for the general case.** You
   narrow the exception from "the whole machine" to "one audited tool."

2. **It is the vendor-designed mechanism, not a workaround.** Canonical built
   the restriction *specifically* to be paired with per-binary AppArmor profiles
   that grant `userns` to programs that legitimately need it (Ubuntu ships
   exactly such profiles for browsers and other sandboxes). The intended end
   state is **"restriction enabled + an explicit, listed allow-profile,"** not
   "restriction defeated." A reviewer sees hardening *on* with a named,
   enumerated exception — the posture a CIS/STIG-style baseline actually wants.

3. **Auditable and declarative.** The exception is a single text file in
   `/etc/apparmor.d/` naming one path and one permission. It is diffable,
   reviewable, version-controllable, and removable, and endpoint/compliance
   tooling can enumerate AppArmor profiles to see exactly what was granted to
   what. That is far easier to assess and sign off than a loosened kernel
   sysctl, which presents only as "a hardening default was turned off" — with no
   indication of why, for whom, or how to scope it back.

4. **You already trust the binary.** In a controlled environment `bwrap` arrives
   through a known channel — a distro package from a signed repo, or a managed
   golden image. Granting a capability to a specific, integrity-checked binary is
   a *bounded, named* trust decision, the same kind already made for any
   privileged helper on the box. It is categorically different from "let any
   unprivileged code on the host reach this kernel feature."

5. **The capability is granted to a tool whose *purpose* is to increase
   isolation.** The user namespace bubblewrap creates is precisely what lets it
   drop the agent into a write-confined, reduced-privilege sandbox. So the net
   effect of the profile is **more** confinement of the agent, not less
   protection of the host. You are not weakening the security model to run
   untrusted code faster; you are paying a narrow, named cost to *gain* a
   sandbox — a framing security teams generally accept.

6. **Reversible and self-limiting.** Delete the profile, reload AppArmor, and
   the exception is gone — no lingering kernel state, nothing else silently
   depending on a loosened global. The change cannot drift into a broader
   posture the way a forgotten sysctl drop-in can.

**The honest caveat.** Even scoped to one binary, unprivileged user namespaces
do expand the kernel surface *that binary* (and code it execs inside the
namespace) can reach — so the residual risk is "a vulnerability in bubblewrap,
or in how the agent drives it." That risk is bounded to a single audited tool
and is the *same* risk every Flatpak and Chrome sandbox on the machine already
accepts. For a security team, **"grant `userns` to the distro's bubblewrap, keep
the host-wide restriction on"** is a routine, defensible exception; **"turn the
host-wide restriction off"** is the one that fails a baseline review.

### The host-wide sysctl (personal dev boxes only)

```bash
sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0           # this boot
echo 'kernel.apparmor_restrict_unprivileged_userns=0' | \
  sudo tee /etc/sysctl.d/99-aida-bwrap-userns.conf                      # persist
```

Fine on a personal machine you fully control. On a managed, shared, or
compliance-bound host **expect IT to say no** — it reverses an Ubuntu hardening
default for the entire system, persistently, and reads to an endpoint/compliance
scan as removing a control. Prefer the per-binary profile above.

### Other options if neither is allowed

- **setuid `bwrap`.** A setuid-root bubblewrap sets up the namespace via its
  setuid bit and needs *no* unprivileged-userns grant at all — sidestepping the
  sysctl/profile entirely. The trade-off: a new setuid-root binary is its own
  review item, and some baselines forbid adding setuid binaries. (Ubuntu's
  package is non-setuid + userns by default; this is distro/packaging-dependent.)
- **Run the agent inside a VM or a managed container** the host already trusts,
  with the container/VM as the isolation boundary (and `os_wrap` off, or used
  inside it). This touches **no host kernel hardening at all** and is often the
  *preferred* answer on locked-down endpoints — the boundary IT already manages
  does the confining.

### What to take to IT

> *"Please allow a **per-binary AppArmor `userns` exception** for
> `/usr/bin/bwrap`, keeping `kernel.apparmor_restrict_unprivileged_userns=1`
> host-wide. bubblewrap is a sandboxing tool (the engine under Flatpak); the
> exception is Canonical's designed mechanism for this; it is scoped to one
> signed binary, auditable, and reversible; and it enables more confinement of
> the agent process, not less protection of the host. Profile attached for
> review."*

Hand them the profile file from the recommended section. That is a routine
exception to approve; the host-wide sysctl is not.

## Enabling the OS sandbox on a new machine

<!-- trace:STORY-665 -->

This is the **single place** to bring a fresh host to a working-confinement
state. The whole sequence is also available as a guided, copy-pasteable printer:

```bash
aida doctor --fix-sandbox
```

It detects the current state of *this* host, prints only the steps that host
actually needs (with sudo steps clearly marked "run this yourself"), and runs a
non-sudo self-test smoke as the final check. It never runs sudo for you.

### Prerequisites

- **Linux.** bubblewrap is Linux-only.
- **The `bwrap` package.** Install it from your distro:
  ```bash
  sudo apt install bubblewrap        # Debian / Ubuntu
  # other distros: install the `bubblewrap` package via your package manager
  ```
- **Unprivileged user namespaces permitted by the kernel** (see next step).

### Step 1 — permit unprivileged user namespaces

On recent Ubuntu (23.10+/24.04) AppArmor blocks unprivileged userns by default,
so even with `bwrap` installed the confinement self-test fails and `os_wrap`
fails-closed. Grant it with **one** of these — see [Permitting unprivileged user
namespaces](#permitting-unprivileged-user-namespaces-the-apparmor-profile-vs-the-sysctl)
for the full rationale, the trade-offs, and the IT framing:

```bash
# RECOMMENDED on any managed / shared host — a scoped per-binary AppArmor
# profile that leaves the host-wide restriction ON for everything else:
sudo tee /etc/apparmor.d/bwrap >/dev/null <<'EOF'
abi <abi/4.0>,
include <tunables/global>

profile bwrap /usr/bin/bwrap flags=(unconfined) {
  userns,
  include if exists <local/bwrap>
}
EOF
sudo apparmor_parser -r /etc/apparmor.d/bwrap

# OR — personal dev box ONLY — flip the host-wide restriction off (a managed/
# compliance-bound IT will object to this; prefer the profile above):
sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
echo 'kernel.apparmor_restrict_unprivileged_userns=0' | sudo tee /etc/sysctl.d/99-aida-bwrap-userns.conf
```

### Step 2 — opt in

The OS sandbox is off by default. Enable it in `.aida/config.toml`:

```toml
[contained]
os_wrap = true
```

> A per-host `AIDA_OS_WRAP=1` environment override is incoming. Until it merges,
> use the `[contained] os_wrap` config knob.

### Step 3 — verify

```bash
aida doctor --fix-sandbox    # step 1 should report a passing self-test
aida config show             # renders the resolved [contained] posture
```

`aida doctor` (and `aida init`) also report bwrap availability inline: a green
check once confinement is ready, or the **exact** install / sysctl remediation
when it is not — with a pointer back to `aida doctor --fix-sandbox`.

## Configuration

All knobs live under `[contained]` in `.aida/config.toml`. There are **no
`AIDA_*` environment variables** for the sandbox posture (the only bwrap-related
env var is `AIDA_REQUIRE_BWRAP_LIVE`, which is CI/test-only — see
[`../environment-variables.md`](../environment-variables.md)).

```toml
# .aida/config.toml
[contained]
os_wrap = true                 # master switch (default false)
# Optional, all require os_wrap = true:
read_allowlist = ["/data/shared"]                 # strict read-confinement (default [])
allowed_hosts = ["github.com", "api.anthropic.com"]  # egress allowlist (default [])
managed_domains_only = true                        # hard default-deny egress (default false)
```

| Knob | What it does | Default |
| --- | --- | --- |
| `os_wrap` | Master switch for the bwrap OS sandbox. | `false` |
| `read_allowlist` | When non-empty, replaces `--ro-bind / /` with an enumerated read-only set (essential toolchain paths + this list + the worktree) so host secrets outside it are simply absent. | `[]` |
| `allowed_hosts` | Network-egress allowlist. Empty = no restriction (full egress), **not** deny-all. | `[]` |
| `managed_domains_only` | Hard default-deny egress (block without prompt) on the headless path. | `false` |
| `enable` (alias of legacy `[agents] contained`) | The **Claude-Code-native** `--settings` sandbox posture — *distinct* from `os_wrap`, which is AIDA's own OS boundary. | `false` |

The per-knob narratives (egress allowlist, managed-domains-only, strict
read-confinement) are in [`../cli/03-work-autonomy.md`](../cli/03-work-autonomy.md).

## Current scope and limitations

- **Headless only.** `os_wrap` wraps the headless drain launches
  (`aida queue work --auto-complete --no-human`, `claude -p`). The interactive
  `aida agent new` launch is **not** wrapped yet — wiring that is TASK-864,
  deferred until unprivileged userns confinement is reliable on the dev host.
  So `os_wrap` confines unattended drains, not keyboard-driven sessions.
- **Write-confinement by default.** The default posture makes the whole host
  filesystem *readable* (read-only); a rogue drain can't *write* outside its
  tree but can still *read* host secrets (`~/.ssh`, `~/.aws`, …) unless you
  tighten reads with `read_allowlist`.
- **Egress is opt-in and orthogonal.** Network restriction is governed by the
  egress knobs, not `os_wrap`. With them unset, a wrapped drain has full
  network egress.

## Discovery path

Two read-only surfaces tell you where you stand before enabling anything:

- **`aida doctor`** reports whether `bwrap` is available on this host —
  installed, confinement-capable, or installed-but-userns-blocked — and prints
  the **exact** copy-pasteable remediation (install command or runtime+persist
  sysctl) for the not-ready states, plus a pointer at the guided setup printer.
- **`aida doctor --fix-sandbox`** prints the full, host-specific bring-up
  sequence (see [Enabling the OS sandbox on a new
  machine](#enabling-the-os-sandbox-on-a-new-machine) above) and runs a non-sudo
  self-test smoke. The single command to run on a fresh machine.
- **`aida config show`** renders the resolved `[contained]` posture, including
  `os_wrap`, `read_allowlist`, `allowed_hosts`, and `managed_domains_only` —
  each with its effective value and source (project config vs. default).

Start there: confirm `bwrap` is confinement-capable via `aida doctor`
(or run `aida doctor --fix-sandbox` for the guided steps), then opt in with
`os_wrap = true` and verify with `aida config show`.

## Performance overhead

**Effectively zero for AIDA's use case.** `bwrap` is a thin namespace wrapper —
not a container runtime or VM. There is no daemon, no image layer, and no
syscall interception. The cost is a **one-time ~9–10 ms at process spawn**
(creating the user namespace + setting up the bind mounts); after `exec` the
agent runs at **native speed**.

Measured on a dev host (Linux, userns enabled):

| | bare | under `os_wrap` (bwrap) | overhead |
|---|---|---|---|
| trivial spawn (`true`, 50×) | 0.47 ms | 9.36 ms | ~9 ms |
| `node --version` (20×) | 4.7 ms | 14.2 ms | ~9.5 ms |

The overhead is **fixed per spawn, not proportional to the work** — note the
delta is the same ~9.5 ms whether the wrapped program does nothing or starts a
runtime. Because `os_wrap` wraps the **outer** agent process once (subprocesses
the agent spawns are already inside the namespace — no re-wrapping) and that
agent then runs for **minutes**, the one-time cost is ~0.005% of a session —
lost in the noise. Specifics:

- **Filesystem:** bind mounts are zero-copy passthrough (no I/O penalty); `/tmp`
  is a tmpfs (RAM-backed, often *faster* for scratch writes).
- **Network:** `os_wrap` is a write-confinement boundary with shared net — no
  network overhead (egress is governed separately).
- **CPU / memory:** native; namespaces are cheap.

The only workload that would notice is one spawning *thousands* of short-lived
wrapped processes — which AIDA does not do (it wraps long-lived agents). For
confining an agent session, enabling `os_wrap` is essentially free.

## Related

- [`../cli/03-work-autonomy.md`](../cli/03-work-autonomy.md) — the per-knob
  narratives and the autonomy/drain context.
- [`../environment-variables.md`](../environment-variables.md) — the `[contained]`
  knob pointer table and the `AIDA_REQUIRE_BWRAP_LIVE` test-only var.
- [`per-agent-config.md`](per-agent-config.md) — `aida agent new` launch config
  (the interactive path TASK-864 will bring under `os_wrap`).
