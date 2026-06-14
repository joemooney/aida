# Per-Agent Launch Config

`aida agent new` can read operator-controlled default flags for each supported
agent from TOML config files.

Config paths:

- User defaults: `~/.aida/agents.toml`
- Project overrides: `.aida/agents.toml`

Merge rule: user defaults are loaded first. If the project config contains the
same agent table, that table's `default_flags` replaces the user list for that
agent. Launch-time `--extra-flag` values are appended after config defaults.

Example:

```toml
[agents.antigravity]
default_flags = ["--dangerously-skip-permissions"]

[agents.codex]
default_flags = ["--ask-for-approval=never", "--sandbox=danger-full-access"]

[agents.claude]
default_flags = []
```

## Faithful launchers + posture knobs

By default, every interactive launcher is *faithful* — it spawns the underlying
tool with that tool's **native** permission/sandbox posture and injects nothing.
For Claude that means `aida session new`, `aida session start --launch`,
`aida queue work`, and `aida agent new claude` no longer inject
`--permission-mode bypassPermissions`; Claude prompts the way bare `claude` does.
Codex and Antigravity were already faithful (their `--bypass-sandbox` opt-in is
unchanged).

To restore bypass posture for the **whole fleet** at once, set a single uniform
knob:

```toml
[agents]
bypass = true        # user base ~/.aida/agents.toml; project .aida/agents.toml overrides
```

To opt into contained Claude Code launches, use:

```toml
[agents]
contained = true
```

Contained mode is mutually exclusive with `bypass`. It launches Claude with
Claude Code's native Bash sandbox enabled, hard-fails if the sandbox is
unavailable, disables unsandboxed command retry, auto-allows project-relative
edits only, and denies known destructive Bash commands. AIDA uses the session
lease worktree as the write boundary.

### OS-boundary wrapper — `[contained] os_wrap` (STORY-612)

Contained mode sandboxes only Claude's *Bash* tool; Edit/Write/MCP run
unconfined and there is no OS boundary around the `claude` process itself. The
opt-in `os_wrap` posture closes that gap by wrapping the **whole headless
`claude -p` process** in [bubblewrap](https://github.com/containers/bubblewrap):

```toml
[contained]
os_wrap = true
# allowed_hosts = ["github.com", "static.crates.io", "registry.npmjs.org"]  # slice-1 egress allowlist
# managed_domains_only = true  # STORY-615: headless default-DENY egress (managed set + allowed_hosts), no approval prompt — default false
```

**Headless default-deny (`managed_domains_only`, STORY-615).** The `allowed_hosts`
allowlist alone *prompts* for a non-listed domain, which a headless `claude -p`
drain can't answer. Set `managed_domains_only = true` to add
`sandbox.network.allowManagedDomainsOnly` so egress is **denied without a prompt**
except to the managed set plus `allowed_hosts`. Default **false** — turn it on
only when the drain's egress is fully covered (else a build that needs
crates.io / github.com / npm is cut off; add those to `allowed_hosts`).

**Write-confinement model.** `bwrap --ro-bind / /` makes the whole filesystem
**readable but read-only**, then rw-binds *only* the code worktree, the
`.aida-store` worktree, and the build/auth caches (`~/.cargo`, `~/.npm`,
`~/.claude`, `~/.claude.json`). So every OS-level *write* by Edit/Write/MCP/Bash
is confined to the worktree — a rogue or prompt-injected drain cannot `rm -rf ~`,
tamper with `~/.ssh`, or scribble outside its tree. **Network stays shared**
(bwrap never `--unshare-net`; `claude` itself must reach `api.anthropic.com`), so
egress is bounded by the slice-1 `[contained] allowed_hosts` allowlist, **not** by
this filesystem confinement.

**Limitation:** reads stay broad — read-exfiltration of host credentials is
bounded by the egress allowlist, not the FS boundary. Strict read-confinement
(a default-absent bind-allowlist) is a tracked follow-up.

**Scope:** wraps the three *headless* (`claude -p`) spawn paths only — interactive
sessions (a human is present) are not wrapped.

**Prerequisite — unprivileged user namespaces.** `bwrap` needs to create an
unprivileged user namespace. On Ubuntu 23.10+/24.04 the AppArmor
`apparmor_restrict_unprivileged_userns` knob blocks this for unprofiled binaries.
`os_wrap` is **fail-closed**: if `bwrap` is missing, or the self-test that
confirms userns works fails, the launch errors with remediation rather than
running unconfined. Remediate with **one** of:

```bash
sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0   # host-wide
# or install an AppArmor profile granting bwrap userns (see /etc/apparmor.d)
```

`os_wrap` can combine with `[contained] enable`, but note that running Claude
Code's own Bash bubblewrap *inside* the outer bwrap nests user namespaces, which
some hardened kernels disallow — `os_wrap` alone already provides the broader OS
boundary.

When `bypass = true` (and the launch has no more-specific override), each
launcher injects that tool's appropriate bypass flag:

| tool        | injected flag                                |
| ----------- | -------------------------------------------- |
| claude      | `--permission-mode bypassPermissions`        |
| codex       | `--dangerously-bypass-approvals-and-sandbox` |
| antigravity | `--dangerously-skip-permissions`             |

`[agents] bypass` coexists with the per-tool `[agents.<tool>] default_flags`
tables in the same file.

Precedence (highest first) — anything above the knob wins, and the knob wins
over the native default:

1. Explicit per-launch flag: `--permission-mode <M>` (claude) / `--bypass-sandbox` (codex, antigravity)
2. `aida queue work` only: `AIDA_PERMISSION_MODE` env, then `.aida/config.toml [behavior] permission_mode`
3. `--no-default-flags` (skips agents.toml entirely → native)
4. Per-tool `[agents.<tool>] default_flags` (overrides the knob for that tool)
5. `[agents] contained = true` (Claude strict sandbox posture)
6. `[agents] bypass = true` (uniform knob → each tool's bypass flag)
7. Otherwise → native posture (nothing injected)

The first time an interactive Claude launch lands on the native default, AIDA
prints a one-time pointer to this knob (suppressed thereafter via a marker under
`~/.aida/`).

Launch controls:

- `aida agent new <agent> --no-default-flags` skips both config files (and the bypass knob) for that launch.
- `aida agent new <agent> --extra-flag <FLAG>` appends one raw flag; repeat it for multiple flags.
- Agent-specific explicit flags such as `--permission-mode` or `--bypass-sandbox` still work and override the knob.
- `aida agent new claude --bg` (detached, no answerable TTY) force-injects bypass so the child can't hang on a prompt; an explicit `--permission-mode` still overrides.

### Safety invariant

The unattended headless drain (`aida queue work --auto-complete --no-human`,
which runs `claude -p`) **always** forces `bypassPermissions` regardless of this
knob — a prompting child has no TTY to answer and would hang the drain forever.
That forcing lives on a separate launch path from the interactive builders, so
the faithful-launcher flip never weakens it.

Safety:

These files are operational defaults, not a permission model. Only enable flags
you are comfortable applying to every supervised launch in that scope. In
particular, unsafe permission or sandbox bypass flags are the operator's
responsibility and should not be enabled casually in shared projects.
