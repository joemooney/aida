---
name: Don't wrap auto-evaled aida subcommands in eval
description: User has a shell function that auto-evals specific aida subcommands — wrapping any of them in eval "$(...)" double-evals and breaks them
type: feedback
propagation: scaffolding-pack
originSessionId: 1bf450af-f9e9-49d0-a098-79696b14cefc
---

When suggesting any of the following aida subcommands to this user, write them as **bare commands**, NOT wrapped in `eval "$(...)"`:

- `aida role enter <name>`
- `aida role end`
- `aida role add <name>`
- `aida dev activate`
- `aida dev deactivate`

**Why:** the user has installed a shell function (via `aida dev shell-init --install`) that intercepts these subcommands and auto-evals them internally:

```bash
aida () {
    local _aida_cmd="${1:-} ${2:-}"
    case "$_aida_cmd" in
        "dev activate" | "dev deactivate" | "role enter" | "role end" | "role add")
            eval "$(command aida "$@")"
        ;;
        *)
            command aida "$@"
        ;;
    esac
}
```

Wrapping any of these in `eval "$(...)"` double-evals: the inner `aida ...` already gets eval'd by the function (correctly setting env/PS1), and the outer eval then receives the human-readable echo output (e.g. `✓ Resumed role: ...`, `Last touched: ...`) and tries to interpret each line as a shell command — producing `✓: command not found`, `bash: syntax error near (`, `Last: command not found`, etc. Caught 2026-05-09 (role enter), 2026-05-10 (dev deactivate).

**How to apply:** Always write these as plain invocations: `aida role enter implementer`, `aida dev deactivate`, etc. The function transparently does the right thing.

**Exception:** if a future aida subcommand emits shell code but isn't in the function's allowlist above, the user WOULD need `eval "$(...)"` for it. Check the function's case list before assuming — and if the new subcommand is similar in shape (PS1 mutation, env export), suggest the user add it to their shell-init.
