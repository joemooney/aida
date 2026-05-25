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

Launch controls:

- `aida agent new <agent> --no-default-flags` skips both config files for that launch.
- `aida agent new <agent> --extra-flag <FLAG>` appends one raw flag; repeat it for multiple flags.
- Agent-specific explicit flags such as `--permission-mode` or `--bypass-sandbox` still work.

Safety:

These files are operational defaults, not a permission model. Only enable flags
you are comfortable applying to every supervised launch in that scope. In
particular, unsafe permission or sandbox bypass flags are the operator's
responsibility and should not be enabled casually in shared projects.
