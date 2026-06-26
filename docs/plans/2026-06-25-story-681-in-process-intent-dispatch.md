# STORY-681 — in-process intent dispatch (self-sufficient `aida tui`)

Date: 2026-06-25
Specs: STORY-681 (parent EPIC-52, EPIC-26)
Status: implemented
Complexity: architecture-class (the launch mechanism)

## Approach

STORY-244 split `aida tui` into a *launcher* (paints a board, exits emitting one
`Intent` line on fd 3) plus a bash `aida-tui` wrapper that read the line, `eval`'d
the command (or ran `claude --resume <id>`), reset the terminal, and re-entered.
BUG-612 then routed bare `aida tui` through that wrapper so it Just Worked — but
it still *required* the shell function and the fd-3 plumbing. Standalone `aida tui`
(no `aida dev shell-init`) was the failing case.

STORY-681 moves the dispatch + re-entry loop **into the `aida tui` process**:

```
loop {
    enter TermGuard (raw + alt screen)
    intent = event_loop(board)        // user picks an action
    drop TermGuard                     // cooked mode + main screen restored
    match plan(intent) {
        Quit       => return
        Run{prog,args} => {
            spawn child inheriting the real terminal; wait
            sanitize terminal (tput-reset equivalent)
        }
    }
}
```

No fd 3, no `aida-tui` shell function, no shell-init prerequisite.

## Decisions

- **Direct `Command` spawn, not `sh -c` / `eval`.** Every Intent payload already
  passes `intent::is_safe_payload` (alphanumerics + a tiny punctuation allow-list,
  no shell metacharacters), so the command is a plain whitespace-tokenised argv.
  Spawning the program directly with its args (inheriting stdio onto the real
  terminal) sidesteps the shell entirely — strictly safer than the wrapper's
  `eval`, and it removes the injection surface the wrapper's allow-list defended.
  A defense-in-depth re-check (`payload_is_dispatch_safe`) still runs before
  `Command::new`, since the in-process path no longer routes through
  `intent::serialize`.
- **fd-3 path kept as an opt-in power-user / legacy hook.** `LauncherOptions.intent_fd`
  became `Option<u32>`. `None` (the new default for bare `aida tui`) = in-process
  loop. `Some(fd)` (via the hidden `--intent-fd` flag) = the STORY-244 single-shot
  emit protocol an external dispatcher consumes. The spec explicitly allowed this
  ("The Intent/fd-3 path may remain an optional power-user hook").
- **Terminal sanitize between child and re-entry.** `term::sanitize_after_child()`
  is the Rust equivalent of the wrapper's `tput reset` — best-effort disable raw
  mode + show cursor so a crashed child can't leave the next launcher entry painting
  over garbage. We run it unconditionally (cheap, idempotent) rather than only on
  non-zero exit.
- **State re-read each iteration.** The loop reloads `.aida/tui-state.json` per pass
  so a `resume:` intent from the prior pass is remembered for dialog-session
  discovery on the next.

## Files (build order)

- `aida-tui/src/dispatch.rs` (new) — `Dispatch` enum, `plan(Intent) -> Dispatch`,
  `run_child(program, args) -> ExitStatus`. The Rust equivalent of the wrapper's
  `case "$_intent"`.
- `aida-tui/src/intent.rs` — `is_safe_payload` made `pub(crate)` for the
  defense-in-depth re-check.
- `aida-tui/src/term.rs` — `sanitize_after_child()`.
- `aida-tui/src/launcher.rs` — `LauncherOptions.intent_fd: Option<u32>`; `run`
  split into `run_in_process_loop` (default) + `run_emit_to_fd` (legacy) sharing
  `run_board_once`; `payload_is_dispatch_safe`.
- `aida-tui/src/lib.rs` — register `mod dispatch`; export it in `__test_only`.
- `aida-cli/src/main.rs` — pass `intent_fd` through (no longer defaults to `3`);
  the `aida()` shell wrapper's `tui` case passes through (no more BUG-612 detour);
  the legacy `aida-tui` function passes `--intent-fd 3` to opt into emit mode.
- `aida-cli/src/cli.rs` — `--intent-fd` / `--launcher` help text refreshed.

## Is the shell wrapper now removable?

**Yes for `aida tui`.** Bare `aida tui` no longer needs the `aida-tui` shell
function or the `aida()` wrapper's `tui` special-case — both were BUG-612
workarounds and are now pass-throughs. The `aida-tui` function is retained as an
opt-in hook over the fd-3 protocol (it now explicitly passes `--intent-fd 3`), so
nothing breaks for users who still source it, but it is no longer a prerequisite.

## Tests

- `dispatch.rs` unit tests: every Intent variant → Dispatch mapping; empty payloads
  rejected; `run_child` spawns/waits/reports status (`true`/`false`); missing program
  errors.
- `aida-tui/tests/launcher_intent.rs`: in-process plan mapping for quit/launch/
  resume/shell + `run_child` status, alongside the preserved fd-3 wire-format tests.

## Verification

- `cargo build -p aida-tui -p aida-cli`
- `cargo fmt --all -- --check`
- `cargo clippy -p aida-tui -p aida-cli -- -D clippy::correctness`
- `bash scripts/glyph-lint.sh --block`
- `env -u AIDA_SESSION_ROLE cargo test -p aida-tui`

## Flag for the lead

- The full spawn→wait→re-enter loop and terminal sanitize are unit/integration
  tested at the seams (plan mapping, child spawn/wait, status), but the
  *interactive* round-trip (raw mode → drop guard → child paints → sanitize →
  re-enter) needs a real TTY and is the SUPERVISED keyboard-verify the spec calls
  for. Worth a manual `aida tui` smoke from a bare shell (no `aida-on`/wrapper)
  before merge.
- `sanitize_after_child` does *not* re-run the alternate-screen leave (the child
  inherited the main screen, never the alt screen) — only raw-mode disable + cursor
  show. If a dispatched child enters its own alt screen and crashes without leaving
  it, the next `TermGuard::enter` re-enters alt screen cleanly anyway, but this is
  the one terminal-state edge I'd want a second pair of eyes on.

## Followups (Slices 2-5)

Fuzzy command line, dispatch-by-kind, spec-id search, polish — per
`docs/plans/2026-06-25-tui-redesign-fuzzy-command-palette.md`.
