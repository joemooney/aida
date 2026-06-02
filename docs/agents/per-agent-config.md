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

## Faithful launchers + the uniform bypass knob (STORY-495)

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
5. `[agents] bypass = true` (uniform knob → each tool's bypass flag)
6. Otherwise → native posture (nothing injected)

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
