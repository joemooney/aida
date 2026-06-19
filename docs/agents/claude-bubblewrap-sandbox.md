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
installed the self-test fails and `os_wrap` fails-closed. Remediate with **one**
of:

```bash
# Host-wide (persist under /etc/sysctl.d/):
sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0

# Or install an AppArmor profile granting bwrap userns (see /etc/apparmor.d).
```

Until userns is permitted, enabling `os_wrap` will (correctly) refuse to launch
drains rather than run them unconfined.

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
  installed, confinement-capable, or installed-but-userns-blocked (with the
  one-line `sysctl` remediation hint).
- **`aida config show`** renders the resolved `[contained]` posture, including
  `os_wrap`, `read_allowlist`, `allowed_hosts`, and `managed_domains_only` —
  each with its effective value and source (project config vs. default).

Start there: confirm `bwrap` is confinement-capable via `aida doctor`, then opt
in with `os_wrap = true` and verify with `aida config show`.

## Related

- [`../cli/03-work-autonomy.md`](../cli/03-work-autonomy.md) — the per-knob
  narratives and the autonomy/drain context.
- [`../environment-variables.md`](../environment-variables.md) — the `[contained]`
  knob pointer table and the `AIDA_REQUIRE_BWRAP_LIVE` test-only var.
- [`per-agent-config.md`](per-agent-config.md) — `aida agent new` launch config
  (the interactive path TASK-864 will bring under `os_wrap`).
