# Shell wrapper security advisory: zsh parameter leak (BUG-2000, BUG-2005)

## Summary

In main/dev builds between BUG-1811 (#2463) and BUG-2000 (#2584), the shell
wrapper installed by `aida dev shell-init --install` used `local -` to scope
options inside the `aida()` wrapper function. In bash this scopes shell
options to the function. In zsh, however, `local -` is evaluated as a bare
`local`, which causes zsh to print every current shell parameter and environment
variable to stdout on each wrapped `aida` invocation.

BUG-2000 resolved this for newly generated wrappers by using
`setopt localoptions` under zsh. BUG-2005 adds the `zsh-localopts` capability
marker to `AIDA_SHELL_WRAPPER` so `aida dev status` and the CLI report
previously installed leaking wrappers as stale.

**Released binaries (v0.15.0 and earlier) are NOT affected.** Only users running
main/dev builds from source between those commits who ran
`aida dev shell-init --install` in zsh are affected.

## Action required for affected zsh users

If you use zsh and ran `aida dev shell-init --install` from a main/dev build
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
