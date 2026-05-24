# Plan: STORY-244 — TUI launcher + bash-wrapper re-entry

Date: 2026-05-24
Specs: STORY-244 (supersedes STORY-241, STORY-242)
Status: Draft
Complexity: ~900 prod LOC + ~400 test LOC across `aida-tui` + `aida-cli`, ~6 commits, risk medium

## Context

`aida tui` ships default-on (STORY-137) and PTY-hosts Claude (STORY-132). On
2026-05-15 the user observed visual contention: Claude is itself a full-screen
TUI and its chrome rows fight with our reserved tab bar / status strip. This is
architectural — Claude is designed to own its terminal — so more PTY rendering
polish will not fix it.

STORY-244 pivots away from PTY-hosting Claude. The TUI becomes a **launcher**:
a full-screen navigator that, when the user picks an action, exits and writes a
single intent line on fd 3. A small bash function (`aida-tui`) dispatches the
intent (typically `aida queue work …` or `claude --resume <id>`), and when the
dispatched command exits, the loop re-launches the TUI. Two TUIs no longer
share the screen; each owns it in turn. Legacy PTY-host mode (STORY-132)
remains behind a config opt-in for users who want concurrent PTY panes.

## Approach

Add a `--launcher` flag to `aida tui` that selects a new launcher mode living
in `aida-tui/src/launcher.rs`. The launcher renders a four-region dashboard
(top tabs / left nav / middle list / right preview + bottom status strip),
re-uses `overlay::fetch` + `aida queue work --list-sessions` for data, and on
user action exits cleanly while writing one intent line to fd 3 (the file
descriptor is configurable via `--intent-fd <N>` for testability). The bash
`aida-tui` wrapper is appended to `SHELL_HELPERS` in `aida-cli/src/main.rs` so
`aida dev shell-init --install` materialises it into `~/.aida/shell-init.sh`.
The non-`--launcher` invocation keeps STORY-132 PTY-host behavior for
back-compat, gated additionally by `[tui] mode = "pty-host"` (default
`"launcher"` once shipped) — so a bare `aida tui` from inside the wrapper is
launcher mode, and a bare `aida tui` from outside still gives PTY-host until
the user flips the config.

### Control flow

```
   bash> aida-tui                                     (shell function loop)
            │
            ▼
   aida tui --launcher --intent-fd 3
            │
            ├── ensure_project_context()              (existing, reuse)
            ├── crash-recovery off (launcher never owns PTYs to recover)
            ├── launcher::run(opts) ────► render Dashboard
            │                              ▲       │
            │                              │       │ keystroke
            │                              │       ▼
            │                              └── refetch on g / re-entry
            │
            ▼ Enter on item / `:` palette
   emit one line to fd 3:
     "launch:aida queue work STORY-X"
     "resume:019e2d4f-..."
     "shell:gh pr view 42"
     "quit"
            │
            ▼
   bash wrapper dispatches via eval / claude --resume
            │
            └── on exit, loop top: aida tui --launcher … (re-entry)
```

### Dashboard layout

```
 ┌──────────────────────────────────────────────────────────────────┐
 │ [ implementer ]  reviewer   dialog                  ← top tabs   │
 ├──────────────┬───────────────────────────────────────────────────┤
 │ Queue        │  STORY-243  ratatui crash on resize             ▲ │
 │ Backlog      │ ▸STORY-244  TUI launcher pivot                    │
 │ History      │  TASK-256   theming                               │
 │ PRs          │  BUG-238    edit followups                        │
 │ Sessions     │                                                   │
 │ ─────────    │  ─── right preview: aida show STORY-244 ─────     │
 │ /aida-drain  │  Type: Story · Status: Approved · Tags: ...       │
 │ New session  │  Description: <wrapped, scrollable>               │
 │ Switch role  │                                                 ▼ │
 ├──────────────┴───────────────────────────────────────────────────┤
 │ role:implementer · queue:3 · dialog:idle    q queue · b backlog ?│
 └──────────────────────────────────────────────────────────────────┘
```

## Decisions

- **Decision: intent transport is fd 3, not stdout.** Launcher renders to
  stdout/stderr like any TUI; the intent line goes to a dedicated fd so render
  bytes can never leak into the shell's `$(…)` capture. Rationale: idiomatic
  for shell-eval'd CLIs and matches how `aida session start` already splits
  human-stderr from machine-stdout. The bash wrapper uses
  `3>&1 1>/dev/tty 2>/dev/tty` to capture fd 3 while letting the TUI paint the
  real terminal directly. `--intent-fd N` overrides for tests.
- **Decision: launcher and PTY-host coexist, launcher is the default.** Add a
  `[tui] mode = "launcher" | "pty-host"` toggle (default `launcher`). When the
  `--launcher` flag is passed, mode is forced launcher; when the flag is
  absent, the config decides. STORY-132's PTY-host code stays intact under
  `mode = "pty-host"`. Rationale: STORY-244 acceptance demands both behaviours
  exist; default is the pivot's new shape.
- **Decision: launcher skips crash recovery (`state::load`).** The launcher
  owns no PTY children, so `.aida/tui-state.json` has nothing to re-attach.
  Bash re-entry is the new continuity mechanism. The state file still exists
  for the PTY-host path. Launcher writes `.aida/tui-state.json` with one new
  field — `dialog_session_id` — for persistent-dialog continuity.
- **Decision: reuse `overlay::fetch` for queue / branch / PR / role data.**
  Already wired (sub-ms cache-only path; `--no-ci` fast first-paint).
  Launcher's middle-list rows for `Queue` come from `OverlayModel.queue.head`,
  `Backlog` queries `aida list --status approved,planned --json` (no JSON form
  today — add one or fall back to text parsing; see decision below),
  `Sessions` uses `aida queue work <scope> --list-sessions` (TASK-112) parsed
  via existing `picker::parse_list_sessions`.
- **Decision: add `aida list --json`.** Backlog / History panes need a
  structured list of specs filtered by status. Add `--json` to the
  `Command::List` handler emitting `[{spec_id,title,type,status,tags}]`. The
  TUI parses it with serde. Smaller surface than wiring a new internal API,
  and gives CLI users a queryable list at the same time.
- **Decision: PRs pane shells out to `gh pr list --json`.** The activity
  panel of the existing overlay does not include a list of PRs, but `gh` is
  already a TUI dep transitively via `actions::PrView`. Parse
  `gh pr list --state open --json number,title,headRefName,statusCheckRollup`.
  Empty list on `gh` failure (offline / unauth) is rendered as a placeholder,
  never an error.
- **Decision: keep Ctrl-A chords as alternates; direct keys are primary.**
  In launcher mode no child consumes keystrokes, so `q b h p s r t ?` route
  directly. `Ctrl-A` chord still routes to the same handlers (muscle-memory
  path the acceptance criteria explicitly preserve). `Esc` / `Ctrl-C` quit.
  `:` opens a vim-style command palette (`:q`, `:role reviewer`, `:resume`).
- **Decision: persistent dialog session id lives in `TuiState`.** Add field
  `dialog_session_id: Option<String>` to `state::TuiState` (back-compatible
  via serde `default`). The launcher reads it on entry to populate the
  `dialog` top tab; selecting that tab emits
  `resume:<id>` (or `launch:aida queue work --role dialog` if no id yet).
- **Decision: ship the wrapper via shell-init only, not as a separate
  script.** Option (c) "both" in the requirement is deferred — the immediate
  audience is AIDA developers and `aida init`'s discipline pack already
  encourages shell-init. Standalone `~/.local/bin/aida-tui` is followups.

## Files (in build-order)

### 1. `aida-cli/src/cli.rs` — `Tui` subcommand gets two new flags

- `Command::Tui`: add `#[clap(long)] launcher: bool` and
  `#[clap(long, value_name = "FD")] intent_fd: Option<u32>`. Keep `scope` and
  `no_recover` as today.
- `QueueCommand::List` / `Command::List`: add `#[clap(long)] json: bool` (the
  Backlog / History panes consume it).

### 2. `aida-cli/src/main.rs` — dispatch + shell wrapper

- `handle_tui_command`: extend signature to
  `(scope, no_recover, launcher, intent_fd)`. When `launcher` is true (or the
  config resolves to launcher mode and no `--launcher` was passed), call a new
  `aida_tui::run_launcher(LauncherOptions { scope, intent_fd: intent_fd.unwrap_or(3) })`;
  otherwise keep calling `aida_tui::run` (PTY-host path).
- Decide PTY-host vs launcher at dispatch using `TuiConfig::load` + the new
  `mode` field (see `aida-tui/src/config.rs` below). The current mode-toggle
  detection lives next to `handle_tui_command` (already where the existing
  cfg shape lives).
- `SHELL_HELPERS` const string: append an `aida-tui()` function. Implement
  the loop body as documented in the STORY (`while true; do intent=$(…); case
  …; esac; done`). Recognise `--once` as a flag the function consumes itself
  (sets a `_once=1` local) so the wrapper exits after one dispatch. Use
  `command aida tui --launcher "$@" 3>&1 1>/dev/tty 2>/dev/tty` to capture
  the intent without sending TUI render bytes through `$(…)`.
- `Command::List` handler (the existing dispatch around line 1202 + the
  `handle_list_*` body further down): when `json` is true, emit
  `serde_json::to_string_pretty` of a `Vec<ListRow>` and return early. New
  struct `ListRow { spec_id, title, req_type, status, tags }` near the
  existing handler — keep it small and inlineable.

### 3. `aida-tui/Cargo.toml` — already has serde / serde_json; no change.

### 4. `aida-tui/src/config.rs` — `mode` field + parser

- `TuiConfig`: add `pub mode: TuiMode` and `enum TuiMode { Launcher, PtyHost }`
  with `Default = Launcher`.
- `TuiConfig::load`: recognise `mode = "launcher"` / `mode = "pty-host"` in
  the `[tui]` section using the existing `scan_tui_section` helper. Unknown
  value falls back to default.
- Unit test: `mode_default_is_launcher`, `mode_parses_pty_host`.

### 5. `aida-tui/src/state.rs` — dialog session id

- `TuiState`: add `#[serde(default)] dialog_session_id: Option<String>`.
- No other change — serde `default` keeps PTY-host's existing state files
  loadable. Add test `state_roundtrips_with_dialog_id`.

### 6. `aida-tui/src/intent.rs` — NEW

- `enum Intent { Quit, Launch(String), Resume(String), Shell(String) }`.
- `fn serialize(intent: &Intent) -> String` — renders the single line written
  to the intent fd. Newline-terminated.
  - `Intent::Quit` → `"quit\n"`
  - `Intent::Launch(cmd)` → `format!("launch:{cmd}\n")`
  - `Intent::Resume(id)` → `format!("resume:{id}\n")`
  - `Intent::Shell(cmd)` → `format!("shell:{cmd}\n")`
- `fn write_to_fd(intent: &Intent, fd: u32) -> io::Result<()>` — uses
  `std::os::fd::FromRawFd` (Unix) to wrap fd into a `File`, write, drop without
  closing the underlying fd. Windows path: `bail!("--intent-fd not supported
  on Windows")` for now — the launcher's primary audience is Unix; Windows
  users get PTY-host until a follow-up.
- Tests: `intent_serializes_each_variant`, `quit_terminates_with_newline`,
  `launch_preserves_inner_command_spaces`.

### 7. `aida-tui/src/nav.rs` — NEW

- `enum NavSection { Queue, Backlog, History, Prs, Sessions, ActionDrain,
  ActionNewSession, ActionSwitchRole }`.
- `struct NavState { sections: Vec<NavSection>, selected: usize }`.
- `fn render(frame, area, state, theme)` — bordered block with one row per
  section, the action verbs separated by a `─────` rule.
- `fn select_next/prev`. Tests: `nav_wraps_both_ways`,
  `nav_default_starts_on_queue`.

### 8. `aida-tui/src/dashboard.rs` — NEW (the four-region layout)

- `enum RoleTab { Implementer, Reviewer, Dialog }` with `cycle_next/prev` and
  `as_str`.
- `struct ListRow { id: String, title: String, status: String, kind: RowKind }`
  with `enum RowKind { Queued, Backlog, History, Pr, Session, Action }`.
- `struct DashboardModel { role: RoleTab, nav: NavState, rows: Vec<ListRow>,
  selected: usize, preview: Vec<String>, ambient: AmbientState }`.
- `struct AmbientState { role: String, queue_depth: usize, dialog_state:
  &'static str }`.
- `fn fetch(role: RoleTab, section: NavSection, dialog_id: Option<&str>) ->
  DashboardModel` — the data-loading entry point. Calls:
  - `Queue`: reuses `crate::overlay::fetch(true)` then takes
    `model.queue.head`.
  - `Backlog`: `Command::new(aida_exe()).args(["list", "--status",
    "approved,planned", "--json"]).output()` → parse the `Vec<ListRow>` shape
    added in step 2.
  - `History`: `aida list --status completed --json` plus a `--limit 50`
    (followup if `--limit` doesn't already exist).
  - `PRs`: `gh pr list --state open --json
    number,title,headRefName,statusCheckRollup`; empty on failure.
  - `Sessions`: `aida queue work <launch_scope> --list-sessions` parsed via
    `crate::picker::parse_list_sessions`.
- `fn render(frame, model, theme)` — `Layout::vertical([Length(1),
  Min(0), Length(1)])` for top-tab row / body / status strip; the body is
  `Layout::horizontal([Length(20), Percentage(40), Percentage(60)])` for
  left nav / middle list / right preview.
- `fn build_preview(row: &ListRow) -> Vec<String>` — shells out to
  `aida show <id>` (existing CLI) and captures into the preview pane.
  Cached per-row for the lifetime of the dashboard.
- `fn ambient_strip(state: &AmbientState, hint: &str) -> String` — direct-key
  hint string + ambient. Reuses the **TASK-282** redundancy helpers when
  available (today they live in `aida-cli/src/main.rs` —
  `sess_anchor_annotation`, `wt_divergence_segment`; promote to a small
  `aida-core` module if the launcher needs them cross-crate, per the spec's
  TASK-282 comment).
- Tests: `dashboard_layout_snapshot`, `dashboard_renders_empty_queue_hint`,
  `dashboard_role_tab_filters_rows`.

### 9. `aida-tui/src/launcher.rs` — NEW (the launcher event loop)

- `pub struct LauncherOptions { pub scope: Option<String>, pub intent_fd: u32 }`.
- `pub fn run(opts: LauncherOptions) -> Result<()>`:
  1. `ensure_project_context` (reuse existing in `lib.rs`).
  2. `TuiConfig::load`, `term::install_panic_hook`, `term::install_signal_handler`.
  3. `term::TermGuard::enter()` — same RAII guard as PTY-host path.
  4. `DashboardModel::fetch(initial_role, NavSection::Queue, dialog_id)`.
  5. Event loop: crossterm event read (no mpsc thread needed — launcher does
     not have PTY readers competing), match on key, mutate model, redraw.
  6. On user action → build `Intent`, call `intent::write_to_fd`, exit guard,
     `Ok(())`.
- `fn route_key(key, model) -> LauncherAction` — pure state machine,
  unit-testable. Returns one of: `Redraw`, `Emit(Intent)`, `RefetchOnly`,
  `Quit`. Direct keys: `q`/`b`/`h`/`p`/`s` switch nav section; `r` cycles
  role tab; `t` opens theme picker (placeholder routing for TASK-256); `?`
  opens help (reuse `crate::help`); `g` refresh; `Enter` acts on selected
  row; arrows + `j/k` move; `Tab` cycles role; `Esc/Ctrl-C/q` quit; `:`
  enters palette mode; `Ctrl-A <key>` is parsed via the existing
  `key_matches` helper for the alternate-binding path.
- `fn act_on_row(row: &ListRow, role: RoleTab) -> Intent` — the per-row
  Enter handler. Translates a `RowKind::Queued`/`Backlog` row into
  `Intent::Launch(format!("aida queue work {}", row.id))`; a `Pr` row into
  `Intent::Shell(format!("gh pr view {}", row.id))`; a `Session` row into
  `Intent::Resume(row.id.clone())`; an `Action` row into the verb's
  appropriate intent (e.g. `/aida-drain-queue` → `Intent::Launch("aida queue
  work --auto-complete")`).
- Tests: `route_key_q_switches_to_queue`, `enter_on_queued_emits_launch`,
  `enter_on_session_emits_resume`, `colon_q_emits_quit`.

### 10. `aida-tui/src/lib.rs` — module wiring + new entry point

- `mod launcher; mod intent; mod nav; mod dashboard;` (alongside existing).
- New `pub fn run_launcher(opts: LauncherOptions) -> Result<()>` calling
  `launcher::run`. Keep existing `pub fn run(opts: TuiOptions)` for the
  PTY-host path.
- `TuiOptions` keeps `scope` + `no_recover`; add `LauncherOptions` separately
  rather than threading a flag through `TuiOptions` — different surfaces.

### 11. `aida-cli/src/main.rs` — dispatch glue (second pass)

- `handle_tui_command`: branch on `launcher` flag OR
  `TuiConfig::load(cwd).mode == TuiMode::Launcher`; call `run_launcher`
  with `intent_fd.unwrap_or(3)`.
- `Command::List` JSON path: as described in step 2.

### 12. Tests landing alongside their modules; integration tests in `aida-tui/tests/launcher_intent.rs` — NEW

- `launcher_writes_quit_intent_to_fd3`: spawn the launcher with pipes wired
  to fd 3 (use `std::os::unix::process::CommandExt::pre_exec` or `nix::unistd`
  to dup the pipe write end onto fd 3 in the child), feed `q`, assert the
  pipe yields `quit\n`.
- `launcher_writes_resume_intent_for_session_row`: prepare a fixture
  `DashboardModel` via constructor and drive `route_key` directly (faster +
  no PTY needed); verify the emitted Intent.
- `launcher_falls_back_to_pty_host_without_flag_when_config_is_pty_host`:
  shell into a temp project with `.aida/config.toml` containing
  `[tui] mode = "pty-host"`, assert that `aida tui` (no flag) does NOT emit
  an intent on fd 3.

### 13. Shell-wrapper test in `aida-cli/tests/shell_wrapper.rs` (or wherever `shell-init` is exercised today)

- `aida_tui_function_dispatches_launch_intent`: write a fake `aida` shim into
  `$PATH` that emits `launch:echo ok` on fd 3 and exits 0; source the
  `SHELL_HELPERS` body in a bash subprocess; invoke `aida-tui --once`;
  assert the echoed `ok` reached stdout.
- `aida_tui_function_loops_until_quit`: same shim, three invocations emit
  `launch:true`, `launch:true`, `quit`; assert loop iterates 3 times.

## Critical Files

- `aida-cli/src/cli.rs`
- `aida-cli/src/main.rs`
- `aida-tui/src/lib.rs`
- `aida-tui/src/launcher.rs` *(new)*
- `aida-tui/src/intent.rs` *(new)*
- `aida-tui/src/nav.rs` *(new)*
- `aida-tui/src/dashboard.rs` *(new)*
- `aida-tui/src/config.rs`
- `aida-tui/src/state.rs`
- `aida-tui/tests/launcher_intent.rs` *(new)*
- `aida-cli/tests/shell_wrapper.rs` *(new, or extend an existing tests file)*

## Reusable helpers (do not reimplement)

- `aida_tui::overlay::fetch(no_ci)` — cache-only AIDA status projection; reuse
  for Queue / role / cache panes.
- `aida_tui::picker::parse_list_sessions` — bulleted `--list-sessions` parser
  for the Sessions pane.
- `aida_tui::welcome::panel` — reused for the Help / empty-state body lines.
- `aida_tui::help::cheatsheet` — reused for `?` overlay (extend with
  launcher's direct keys).
- `aida_tui::term::TermGuard`, `install_panic_hook`, `install_signal_handler`
  — same RAII teardown the PTY-host path uses (BUG-110 protection).
- `aida_tui::app::describe_key_long`, `encode_key`, `key_matches` — keystroke
  parsing for the alternate `Ctrl-A` chord path.
- `aida_tui::state::{save, load, clear}` — persistence layer; extend with
  `dialog_session_id`.
- `aida_tui::config::TuiConfig::load` + `scan_tui_section` —
  config-file scanner; extend for `mode = "..."`.
- `aida_cli::punt::{write_signal, read_signal}` precedent — file-handshake
  pattern; the intent fd is the same shape (one-shot artifact written by the
  child, consumed by the parent).
- `aida_cli::main::sess_anchor_annotation`, `wt_divergence_segment`
  (TASK-282) — redundancy rules for the ambient strip; promote to `aida-core`
  if cross-crate.
- `aida_tui::picker::PickerState` — used as-is for the legacy PTY-host
  picker; do not reuse for the launcher's middle list (different shape).

## Risks + gotchas

1. **Risk**: fd 3 swallowed when the user runs `aida tui --launcher` from
   their shell without the wrapper. The child writes to fd 3, fd 3 is the
   inherited stderr or stdout, and intent bytes spray into the user's
   terminal after the alt-screen restores. **Mitigation**: at launcher
   start-up, `fstat(3)` (`is_fd_open`) — if fd 3 looks like a regular file or
   pipe, OK; if it is the same device as fd 1 / fd 2, refuse to emit and
   print a one-line hint pointing at `aida dev shell-init --install`.
2. **Risk**: the bash wrapper double-loops or fails to capture fd 3 on shells
   other than bash 4+ (zsh, dash). **Mitigation**: gate the shell helper on
   `case $SHELL` (matching the existing rc-detection in
   `handle_dev_shell_init`). Document zsh support as Phase 1; dash / fish are
   followups.
3. **Risk**: a hosted child (`aida queue work …`) that crashes or is killed
   mid-run leaves the user in the wrapper loop with a dirty terminal. Re-
   entry into the TUI immediately enters raw mode again over a corrupted
   screen. **Mitigation**: the bash wrapper emits `tput reset` between
   `eval` and re-launch when the dispatched command's exit code is non-zero
   AND the command was a `launch:` intent (Claude-bearing).
4. **Risk**: shell-init regenerates wholesale on every `--install`, so a user
   who customized their shell-init keeps losing edits. **Mitigation**:
   `SHELL_HELPERS` is generated wholesale today by design — the migration
   path is documented. We follow the same pattern (no change in user
   expectation). The split-out helpers file at `~/.aida/shell-init.sh`
   already exists so user rc files stay tiny.
5. **Risk**: `aida list --json` becomes a public API surface and an
   unstable JSON shape later breaks the launcher. **Mitigation**: hide
   `--json` flag from `--help` with `#[clap(hide = true)]`; document in the
   code that the schema is internal to the launcher; bump a `schema_version`
   field in the emitted object so future readers can detect mismatch.
6. **Risk**: launcher state persistence (`dialog_session_id`) collides with
   PTY-host's `TuiState.tabs` on disk — a user toggling modes corrupts the
   other path. **Mitigation**: keep both fields in `TuiState` and gate each
   path's `clear`/`save` calls to their own field, so the launcher never
   wipes `tabs` and PTY-host never wipes `dialog_session_id`.
7. **Risk**: `gh pr list` is slow / unauthenticated / offline and stalls the
   PRs pane. **Mitigation**: fetch on first `p` keypress (not at launcher
   start), display "loading…" placeholder, time out at 5s, render an empty
   list + a one-line "(`gh` failed: …)" footer on failure (matching
   `overlay::fetch` refreshing behaviour).
8. **Risk**: the launcher's `Enter` on a `Queued` row emits `launch:aida
   queue work STORY-X` which the wrapper `eval`s — shell metacharacters in
   the spec id (impossible today, but defense in depth) would inject. Same
   for `:resume` and `shell:`. **Mitigation**: the intent serializer
   validates the inner command against
   `[A-Za-z0-9._/= \-]+` and refuses (emits `quit` + prints a hint) on a
   character class miss. Tested in `intent::serialize_rejects_metacharacters`.
9. **Risk**: launcher's `term::install_signal_handler` is process-global and
   conflicts with whatever the dispatched child installs (Claude installs
   its own SIGINT handler). **Mitigation**: `TermGuard::drop` runs before
   the launcher process exits and the bash wrapper invokes the child fresh
   each time — no shared handler state crosses the boundary. Document this
   in launcher.rs.

## Tests (named, not "add tests")

Module-level:
- `aida_tui::intent::intent_serializes_each_variant`
- `aida_tui::intent::serialize_rejects_metacharacters`
- `aida_tui::intent::write_to_fd_round_trips_through_a_pipe`
- `aida_tui::nav::nav_wraps_both_ways`
- `aida_tui::dashboard::dashboard_layout_snapshot`
- `aida_tui::dashboard::dashboard_role_tab_filters_rows`
- `aida_tui::dashboard::ambient_strip_applies_task_282_redundancy_rules`
- `aida_tui::launcher::route_key_q_switches_to_queue`
- `aida_tui::launcher::enter_on_queued_emits_launch_intent`
- `aida_tui::launcher::enter_on_session_emits_resume_intent`
- `aida_tui::launcher::colon_q_palette_emits_quit`
- `aida_tui::launcher::ctrl_a_q_alternate_emits_quit`
- `aida_tui::launcher::g_refetches_dashboard`
- `aida_tui::launcher::r_cycles_role_tab`
- `aida_tui::config::mode_default_is_launcher`
- `aida_tui::config::mode_parses_pty_host`
- `aida_tui::state::state_roundtrips_with_dialog_id`

Integration:
- `aida_tui_tests::launcher_writes_quit_intent_to_fd3` (spawns the binary)
- `aida_tui_tests::launcher_refuses_when_fd3_is_a_terminal`
- `aida_cli_tests::aida_tui_function_dispatches_launch_intent`
- `aida_cli_tests::aida_tui_function_loops_until_quit`
- `aida_cli_tests::aida_tui_function_once_flag_exits_after_one_dispatch`
- `aida_cli_tests::handle_tui_command_pty_host_when_config_is_pty_host`
- `aida_cli_tests::handle_tui_command_launcher_when_flag_overrides_pty_host_config`

## Verification

```bash
set -euo pipefail
TMP=$(mktemp -d); cd "$TMP" && git init -q && aida init --no-skills --no-hooks
aida add --title "smoke story" --type story --status approved

# 1. Build the launcher binary.
( cd /home/user/aida && cargo build -p aida-cli --features tui )
AIDA_BIN=/home/user/aida/target/debug/aida

# 2. Launcher writes a `quit` intent to fd 3 on `q`.
( printf 'q' | $AIDA_BIN tui --launcher --intent-fd 3 3>"$TMP/intent.txt" >/dev/null 2>&1 ) || true
grep -qx 'quit' "$TMP/intent.txt" || { echo "FAIL: launcher did not emit quit"; exit 1; }

# 3. Launcher writes a `launch:aida queue work …` intent on Enter on the queued
#    smoke STORY. Simulate by sending `\n` (Enter); the launcher's default
#    nav is Queue, default selection is row 0.
( printf '\n' | $AIDA_BIN tui --launcher --intent-fd 3 3>"$TMP/intent.txt" >/dev/null 2>&1 ) || true
grep -q '^launch:aida queue work STORY-' "$TMP/intent.txt" \
    || { echo "FAIL: enter did not emit launch intent"; exit 1; }

# 4. Bash wrapper dispatches the launch intent and re-enters once.
$AIDA_BIN dev shell-init --install
. ~/.aida/shell-init.sh
# Use --once to short-circuit the loop after one dispatch.
LAUNCHED=$(aida-tui --once <<<'\n' 2>&1 | tail -1)
echo "$LAUNCHED" | grep -q 'queue work' || { echo "FAIL: wrapper did not dispatch"; exit 1; }

# 5. PTY-host preserved when `[tui] mode = "pty-host"` is set and `--launcher`
#    is NOT passed.
mkdir -p .aida && printf '[tui]\nmode = "pty-host"\n' >> .aida/config.toml
# A bare `aida tui` without --launcher in PTY-host mode must NOT touch fd 3.
( $AIDA_BIN tui 3>"$TMP/intent2.txt" </dev/null >/dev/null 2>&1 & echo $! > "$TMP/pid"; sleep 1; kill "$(cat $TMP/pid)" ) || true
[ ! -s "$TMP/intent2.txt" ] || { echo "FAIL: PTY-host wrote to intent fd"; exit 1; }

# 6. Negative: launcher refuses to emit when fd 3 is a TTY (no wrapper).
set +e
$AIDA_BIN tui --launcher </dev/null 2>"$TMP/err.txt"
RC=$?
set -e
grep -q 'shell-init' "$TMP/err.txt" || { echo "FAIL: missing wrapper hint"; exit 1; }
[ $RC -ne 0 ] || { echo "FAIL: expected non-zero exit when fd 3 is a tty"; exit 1; }

echo "OK"
```

## Followups

- TASK: scaffold standalone `~/.local/bin/aida-tui` POSIX script for
  binary-tarball installs that did not run `aida dev shell-init --install`.
- TASK: theming hooks (TASK-256) plumbed through `nav::render` and
  `dashboard::render` once the palette landing lands.
- TASK: launcher mouse support — middle-list click selects row, double-click
  acts as Enter.
- TASK: middle-list search/filter (`/` opens a filter prompt over the list).
- TASK: auto-refresh in launcher mode without keypress (poll every 5s with a
  `Tick` event source).
- TASK: cross-project dialog continuity — dialog session id keyed on project
  root today; later, a `~/.aida/dialog-sessions.json` index for follow-me.
- TASK: Windows support for `--intent-fd` (use named pipe / temp file
  fallback).
- TASK: fish + dash variants of the `aida-tui` wrapper.
- TASK: hide `aida list --json` from `--help` becomes formal CLI surface,
  document JSON schema with a `schema_version` field.
- TASK: salvage merge-confirmation modal UX + PR/lease parsing helpers from
  closed PR-45 (`epic-64` branch) once we render PRs / leases in the
  launcher's middle list.
- TASK: promote `sess_anchor_annotation` / `wt_divergence_segment` from
  `aida-cli/src/main.rs` to a small `aida-core` module so the launcher's
  ambient strip can reuse them without crate-circular dependencies
  (criterion 7 of TASK-282).
- TASK: launcher-mode `aida-tui` keyboard cheatsheet entry in
  `docs/tui/README.md` and the `aida-discipline` pack.

## Related

- **Builds on**: STORY-132 (PTY-host shell — kept under config flag),
  STORY-133 (status overlay — `overlay::fetch` reused), STORY-134 (multi-tab
  picker — `picker::parse_list_sessions` reused), STORY-135 (crash recovery
  — `state` module extended), STORY-137 (default-on), BUG-109 (welcome panel
  / help cheatsheet — reused), BUG-110 (SIGTERM handler — reused), TASK-112
  (`claude --resume`), TASK-282 (statusline redundancy helpers).
- **Supersedes**: STORY-241 (TUI workflow loop), STORY-242 (persistent
  dialog tab) — same goals, PTY-tab architecture.
- **Blocks**: TASK-256 (theming integrates with the new four-region layout),
  BUG-110 follow-up (launcher exits / re-enters constantly, signal handler
  idempotence matters even more).
- **See also**: closed PR-45 on branch `epic-26-4` (rejected PTY-host
  implementation; salvageable helpers documented in the spec's PR-45
  comment).
