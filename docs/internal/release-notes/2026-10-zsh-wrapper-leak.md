# Shell wrapper security advisory: zsh parameter leak

## Summary

In development builds from git main between October 8 and October 12, 2026,
the shell wrapper installed by `aida dev shell-init --install` used `local -` to
scope options inside the `aida()` wrapper function. In bash, this idiom scopes
shell options to the function. In zsh, however, `local -` is evaluated as a bare
`local`, which causes zsh to print every current shell parameter and environment
variable to stdout on each wrapped `aida` invocation.

The wrapper now scopes options using `setopt localoptions` under zsh. In
addition, the `zsh-localopts` capability marker was added to `AIDA_SHELL_WRAPPER`
so that `aida dev status` and the CLI detect previously installed leaking
wrappers as stale and prompt for a refresh.

**Released binaries (v0.15.0 and earlier) are NOT affected.** Only users running
development builds built from git main during this window who ran
`aida dev shell-init --install` in zsh are affected.

## Action required for affected zsh users

If you use zsh and ran `aida dev shell-init --install` from a development build
during this window:

1. **Update and refresh your installed shell wrapper:**
   ```bash
   aida dev shell-init --install
   ```
   Then open a new shell (or re-source `~/.zshrc`). Confirm with:
   ```bash
   aida dev status
   ```
   which should report `Shell wrapper: current`.

2. **Rotate sensitive credentials:**
   Any secret environment variables or shell parameters (API keys, tokens,
   passwords) exported in shells where the leaking wrapper was active may have
   been printed to stdout and recorded in terminal scrollback, command logs,
   or session transcripts. Please rotate any credentials exposed in those
   environments.
