# Plan: TASK-294 — `aida-worker` bash function (MVP queue-drain loop)

Date: 2026-05-19
Specs: TASK-294
Status: Draft
Complexity: ~250 prod LOC, ~140 test LOC, 3 commits, risk medium (process-group kill is the one genuine unknown)

<!--
  Plan for TASK-294. The spec grew via two comments the user explicitly asked
  to "fold into acceptance at pickup": the 2026-05-16 watchdog and the
  2026-05-18 directive-channel refinement. Both are in scope. Slice 1 is the
  shippable tracer bullet; slices 2-3 are the visibility layer the 2026-05-18
  comment calls "the load-bearing requirement".
-->

## Approach

Ship a `aida-worker` shell function plus a small `aida worker` subcommand tree
that makes the function's control channel inspectable. The worker is a loop:
each iteration it reads the head directive from `.aida/worker.cmd` (a FIFO,
one directive per line), acts on it, and re-reads. A bare/absent file means
`drain` — pick the queue head and run a full `aida queue work --auto-complete`
lifecycle, wrapped in `timeout` so a hung or walked-away session can't block
forever. `pause` sleeps and re-checks; `exit` returns 0; anything unknown is
treated as `pause` (defensive). A `drain <args>` line (e.g.
`drain batch:autonomy-modes --zen`) runs a *scoped* drain and is consumed when
it finishes, so a user can write a whole overnight plan into the file as a
heredoc and walk away. The control channel is deliberately **separate from the
work queue** (imperative directives vs declarative specs) but gets **equal
visibility**: `aida worker directives` lists what's pending, and the same
pending directives surface in `aida status` and `aida drain status`.

The function itself is emitted by `aida dev shell-init` (it lives in a Rust
string constant alongside the existing `aida()` wrapper — the acceptance
criterion names a `shell-init.sh` template file under `aida-core/templates/`,
but no such file exists; that path is historical). The `aida worker`
subcommand mirrors the `aida drain` (STORY-301) pattern exactly: dispatched
before storage init, reads only a `.aida/` runtime file, has a `--json` mode.

### Diagram

```
                          aida-worker  (loop)
  ┌─────────────────────────────────────────────────────────────┐
  │  read head line of .aida/worker.cmd  (FIFO; absent → "drain") │
  └───────────────┬───────────────────────────────────────────────┘
                  │
   ┌────────┬─────┴──────┬───────────────┬──────────────────┐
 "exit"   "pause" /     "drain"        "drain <args>"     (queue empty /
   │      unknown      (head drain)    (scoped drain)      nothing drivable)
   │        │             │                │                    │
 return 0  sleep 30s    timeout … aida queue work [args] --auto-complete
           re-check       │                │                    │
           (line stays)   ├── exit 0   → log "shipped"; consume drain line
                          ├── exit 124 → log "TIMED OUT"; worker.cmd ← "pause"
                          ├── "nothing to drive" → sleep 30s (NOT a failure)
                          └── other ≠0 → log "halted";   worker.cmd ← "pause"
```

## Decisions

- **Directive file is a FIFO, one directive per line; `drain` lines are
  consumed on completion, control lines (`pause`/`exit`/unknown) persist.**
  Rationale: the 2026-05-18 comment's concrete need is "chain the next batch
  while the current drain runs" and it uses the word *appended*. A FIFO lets a
  user express a whole nightly plan in one heredoc —
  `printf 'drain batch:B --zen\ndrain batch:C\nexit\n' > .aida/worker.cmd` —
  runs B, runs C, stops. A "last line wins" model (rejected) is simpler but
  only chains one step ahead and makes `aida worker directives`' plural
  "pending directives" meaningless. Control lines persist because `pause` is a
  *state* to re-check, not a unit of work to consume.

- **Consume-after-completion, with an append-safe pop.** When a `drain <args>`
  line finishes, the worker re-reads `worker.cmd` fresh and drops the first
  line. Re-reading (rather than caching the file from the top of the
  iteration) means directives the user appended *during* the multi-minute
  drain survive. Rationale: surviving a worker crash (directive re-runs on
  restart) beats consume-before-run; the residual race (user *overwrites* the
  file mid-drain) is documented, not engineered around — single-user MVP.

- **Empty / "nothing to drive" is a sleep, not a pause.** `aida queue work
  --auto-complete` `bail!`s with a message containing the literal
  `nothing to drive` for both a genuinely empty queue and an all-in-flight
  queue (see `resolve_auto_complete_head`). The worker greps the captured
  output for that substring: match → sleep 30s and re-check (the queue may
  fill from another session); any *other* non-zero → real failure → auto-pause.
  Rationale: pausing on an empty queue would wedge the worker the moment it
  drains everything — the opposite of "drain what I can".

- **Watchdog via `timeout`, default 1800s, knob `AIDA_WORKER_SPEC_TIMEOUT`.**
  Straight from the 2026-05-16 comment's implementation hint. Exit 124 →
  "TIMED OUT" + auto-pause; other non-zero → "halted" + auto-pause. Applies
  even in today's interactive mode (a walked-away user's Ctrl+D-blocked
  session gets killed).

- **No `--no-human` flag baked into the function — it rides the directive
  line.** `drain --no-human` → `aida queue work --no-human --auto-complete`;
  `drain batch:X --zen` → `aida queue work batch:X --auto-complete --zen`. The
  directive line *is* the configuration channel, so no separate env var. Bare
  `drain` stays interactive, matching the spec's "today" framing.

- **`aida worker directives` is a new top-level `Command::Worker` subcommand,
  dispatched pre-storage.** Mirrors `Command::Drain`/STORY-301 exactly: reads
  only `.aida/worker.cmd`, no requirement store, has `--json`. A dedicated
  `worker.rs` module holds the parse/render so it is unit-testable without a
  storage fixture. `aida worker` (subcommand) and `aida-worker` (shell
  function) are deliberately paired names — the subcommand inspects the file
  the function consumes.

- **Heartbeat (`.aida/worker.heartbeat`) is deferred to a followup.** The
  2026-05-16 comment marks it explicitly OPT-IN / "not strictly required", and
  the `timeout` watchdog already covers the dominant hang case. Listed under
  Followups, not built now.

## Files (in build-order)

### Slice 1 — the `aida-worker` shell function + watchdog + scoped drain (tracer bullet)

#### `aida-cli/src/main.rs` — emit the function

- Add `const WORKER_FUNCTION: &str` next to `SHELL_HELPERS` — the `aida-worker`
  bash function. Behavior: loop; read first non-blank line of
  `.aida/worker.cmd` (locate the project root by walking up for `.aida/`);
  `case` on the first word — `exit` → `return 0`; `pause`/unknown → log,
  `sleep 30`, continue (line persists); `drain` → run the drain. The drain
  branch: build the `aida queue work` argv from the directive's remaining
  words (empty → head pickup), wrap in
  `timeout "${AIDA_WORKER_SPEC_TIMEOUT:-1800}" command aida queue work … --auto-complete`,
  capture combined output, `case $?` per the diagram. On success of a scoped
  (`drain <args>`) line, pop the head line; on failure/timeout overwrite the
  file with `pause`; on "nothing to drive" `sleep 30`. Log every action with
  the `aida-worker:` prefix.
- `fn handle_dev_shell_init`: concatenate `WORKER_FUNCTION` into `helpers_body`
  after `SHELL_HELPERS` so `~/.aida/shell-init.sh` carries it. No rc-file
  logic changes — the source-stub already picks it up.

#### `docs/autonomous-drain.md` — document the worker

- Add an `## aida-worker` section: the function, the `.aida/worker.cmd`
  directive vocabulary, the heredoc "overnight plan" example, the
  `AIDA_WORKER_SPEC_TIMEOUT` knob. This is the canonical autonomy guide; the
  worker belongs in it.

### Slice 2 — `aida worker directives` (the visibility command)

#### `aida-cli/src/worker.rs` — NEW: directive-file model

- `const WORKER_CMD_FILE: &str = "worker.cmd"` and
  `fn worker_cmd_path(project_root: &Path) -> PathBuf`.
- `struct Directive { verb: String, args: Vec<String>, raw: String }`.
- `fn parse_directives(path: &Path) -> Vec<Directive>` — read the file, skip
  blank lines and `#` comments, split each line into verb + args. Absent file
  → empty vec.
- `fn render_human(&[Directive]) -> String` and
  `fn render_json(&[Directive]) -> String` (serde).

#### `aida-cli/src/cli.rs` — the subcommand

- `enum WorkerCommand { Directives { #[clap(long)] json: bool } }` with a
  doc comment in the STORY-301 `DrainCommand` style.
- Add `Worker(WorkerCommand)` to `enum Command`.

#### `aida-cli/src/main.rs` — dispatch + handler

- `mod worker;` at the top with the other module declarations.
- Pre-storage dispatch beside the `Command::Drain` block:
  `if let Command::Worker(cmd) = &cli.command { return handle_worker_command(cmd); }`.
- Add `Command::Worker(_) => unreachable!(...)` arms to the two post-storage
  match blocks that already carry `Command::Drain(_) => unreachable!(...)`.
- `fn handle_worker_command(&WorkerCommand) -> Result<()>` — resolve project
  root, `worker::parse_directives`, print human or `--json`. "No pending
  directives." (exit 0) when empty.

### Slice 3 — surface pending directives in status output

#### `aida-cli/src/main.rs` — status surfacing

- `fn handle_drain_command` (`DrainCommand::Status`): after the drain summary
  (including the `No drain in progress.` path), append a one-line pending-
  directive summary read via `worker::parse_directives`.
- `fn handle_status_command_distributed`: near `print_status_queue_section`,
  add a compact "Worker directives: N pending (next: <verb>)" line when the
  file is non-empty. Skip silently when empty so quiet projects stay quiet.

## Critical Files

- `aida-cli/src/main.rs`
- `aida-cli/src/cli.rs`
- `aida-cli/src/worker.rs` (new)
- `docs/autonomous-drain.md`
- `tests/test_aida_worker.sh` (new)

## Reusable helpers (do not reimplement)

- `aida-cli/src/drain_state.rs` — the *template* for this whole effort: a
  `.aida/` runtime file, a pre-storage `aida drain` subcommand, `probe` /
  `render_human` / `render_json`. Copy its shape for `worker.rs`; do not
  invent a different one.
- `SHELL_HELPERS` const + `fn handle_dev_shell_init` (`aida-cli/src/main.rs`) —
  the existing shell-helper emission path; `WORKER_FUNCTION` plugs into the
  same `helpers_body` `format!`.
- `resolve_auto_complete_head` (`aida-cli/src/main.rs`) — already produces the
  `nothing to drive` message the worker greps for; do not add a new empty-queue
  signal.
- `find_aida_repo_above` / project-root resolution already used in
  `handle_dev_shell_init` and `find_main_worktree_root` — reuse for locating
  `.aida/` from the Rust side.
- `aida_core::write_atomic` (TASK-331) — if the *worker subcommand* ever writes
  the file; the bash function uses plain redirection. The Rust reader only
  reads, so no write path is needed in slices 2-3.
- `command aida …` — the shell function MUST call `command aida` (not bare
  `aida`) so it bypasses the `aida()` wrapper and gets raw stdout/exit codes.

## Risks + gotchas

1. **Risk**: `timeout` may not kill the spawned `claude` (and its children).
   GNU `timeout`, run without `--foreground`, puts the command in its own
   process group and signals the whole group — but if `aida queue work` or
   `claude` calls `setsid`/forks into a *new* group, orphans survive a
   timeout. **Mitigation**: the implementer must verify with a real
   forced-timeout test (low `AIDA_WORKER_SPEC_TIMEOUT`, confirm no stray
   `claude` in `ps`). If orphans survive, fall back to `timeout --kill-after`
   plus an explicit `pkill -P`, or `setsid` + tracked-PGID `kill -- -PGID`.
   This is the one genuine unknown — surface it, don't hand-wave it.

2. **Risk**: a real `--auto-complete` lifecycle that blocks on CI can exceed
   the 1800s default, so the watchdog kills legitimate in-progress work.
   **Mitigation**: keep the spec's stated 1800s default but document
   `AIDA_WORKER_SPEC_TIMEOUT` prominently in `docs/autonomous-drain.md` —
   a CI-blocking drain wants it raised (e.g. 5400s).

3. **Risk**: file race — the user overwrites `worker.cmd` (`>`) while the
   worker is mid-drain and about to pop the head line; the worker drops the
   user's *new* head instead of the consumed directive. **Mitigation**:
   re-read fresh immediately before the pop and drop only line 1; document
   that mid-drain edits should *append* (`>>`), and overwrite (`>`) only when
   paused or between specs. Acceptable single-user MVP residue.

4. **Risk**: bash vs zsh — `shell-init.sh` is sourced by either. **Mitigation**:
   stick to the common subset already proven by `SHELL_HELPERS` — `local`,
   `case`, `while`, `[ ]`, `$(…)`. No bash-only `[[ ]]` regex, no arrays that
   differ across shells. (Note: this is *not* the `/bin/sh` hook constraint —
   the rc file runs under the user's interactive bash/zsh.)

5. **Risk**: the worker reads `.aida/worker.cmd` but is launched from a
   sibling worktree where `.aida/` is the per-clone copy. **Mitigation**: the
   function resolves the project root by walking up for a `.aida/` directory
   (same approach as `aida drain status`, which deliberately reads the *main*
   worktree's state); document that the worker should be run from the main
   checkout.

6. **Gotcha**: `.aida/worker.cmd` needs no `.gitignore` change — the
   deny-by-default `.aida/*` rule already ignores it. Adding a `!.aida/...`
   line would be wrong (it is per-clone runtime state). Acceptance item
   "gitignored" is satisfied by doing nothing.

## Tests (named, not "add tests")

Rust unit tests in `aida-cli/src/worker.rs`:
- `parse_directives_absent_file_is_empty` — no file → empty vec.
- `parse_directives_bare_drain` — single `drain` line, no args.
- `parse_directives_scoped_drain_keeps_args` — `drain batch:x --zen` →
  verb `drain`, args `["batch:x", "--zen"]`.
- `parse_directives_fifo_order_preserved` — multi-line file, order intact.
- `parse_directives_skips_blanks_and_comments` — blank lines and `#` lines
  dropped.
- `parse_directives_pause_and_exit` — control verbs parsed.
- `render_json_round_trips` — `render_json` output deserializes back.

Shell integration test `tests/test_aida_worker.sh` (pattern of
`tests/test_distributed.sh`):
- `exit` directive → function returns 0.
- unknown directive → logs, treated as pause (no drain attempted).
- failure injection (stub `aida` returning non-zero) → `worker.cmd`
  overwritten with `pause`.
- `nothing to drive` output → worker sleeps, does NOT pause.
- FIFO consume — a `drain <args>` head line is gone after the drain returns 0.

## Verification

```bash
TMP=$(mktemp -d); cd "$TMP" && git init -q && aida init >/dev/null
source ~/.aida/shell-init.sh                       # picks up aida-worker

# Function exists and exits cleanly on the `exit` directive.
echo exit > .aida/worker.cmd
aida-worker; echo "exit-rc=$?"                      # expect: exit-rc=0

# Visibility command reads the FIFO.
printf 'drain batch:b --zen\ndrain batch:c\nexit\n' > .aida/worker.cmd
aida worker directives                              # expect: 3 pending, next=drain
aida worker directives --json | jq '.[0].verb'      # expect: "drain"

# Pending directives surface in status output.
aida drain status   | grep -i directive             # expect: pending-directive line
aida status         | grep -i 'worker directive'    # expect: "N pending"

# Empty file → no directives.
: > .aida/worker.cmd
aida worker directives                              # expect: No pending directives.
```

## Followups

- Opt-in heartbeat: worker touches `.aida/worker.heartbeat`, kills a child whose mtime goes stale.
- `aida worker` controls beyond `directives` — e.g. `aida worker pause` / `aida worker stop` as typed wrappers over editing `worker.cmd`.
- Promote EPIC-30 if the file-directive channel hits real multi-user / multi-machine limits.
- Raise or auto-tune `AIDA_WORKER_SPEC_TIMEOUT` once headless `--auto-complete` durations (incl. CI waits) are measured.

## Related

- Builds on: STORY-263 (`--no-human`, completed), TASK-292 / TASK-293 (head + `next` pickup, completed), STORY-301 (`drain-state.json` — the module pattern this mirrors, completed).
- Composes with: STORY-276 (headless implementer — when it lands, `drain --no-human=both` becomes fully autonomous).
- Next step if it grows limits: EPIC-30 (full worker daemon, Draft).
- See also: `docs/autonomous-drain.md`.
