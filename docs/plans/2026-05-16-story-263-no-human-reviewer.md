# Plan: STORY-263 — `--no-human` headless reviewer (first cut)

Date: 2026-05-16
Specs: STORY-263
Status: In Progress
Complexity: ~180 prod LOC, ~80 test LOC, 1 commit, risk low

<!--
  Reviewer-only first cut of STORY-263. Authoritative scope is STORY-263's
  `## Acceptance` section; the 22:03 scope-expansion comment is reference,
  not binding. Headless implementer (STORY-276), skill audit (TASK-297),
  and the stream-json watchdog (TASK-298) are out of this PR.
-->

## Approach

Add a `--no-human[=MODE]` flag to `aida queue work`. The flag turns the
launched Claude session from an interactive `exec claude` into a headless
`exec claude -p` — single-turn, exits on its own, no Ctrl+D. The
`--auto-complete` orchestrator spawns its phase subprocesses already; under
`--no-human` it appends `--no-human` to the phase-3 reviewer subprocess
(`aida queue work PR-N`), so the reviewer runs headless. Phase 1 (implementer)
stays interactive in this cut — the headless implementer is STORY-276 and
needs `/aida-punt`. The reviewer is the SPIKE-7-blessed "safe first cut": it
already writes its verdict to `AIDA_REVIEW_VERDICT_FILE` and stops before any
merge (skill step 7a), so the orchestrator's existing verdict-file handshake
+ phase-4 merge need no change — the reviewer just stops needing a keystroke.

Headless launch flags are exactly SPIKE-7's mandatory set:
`claude -p "<prompt>" --permission-mode bypassPermissions --output-format
stream-json --verbose --session-id <uuid>` — never `--bare`. Claude's
stream-json stdout is redirected to `<main-root>/.aida/headless-logs/<scope>-<uuid>.jsonl`
so (a) the orchestrator's own stdout stays clean (it matters under `--json`)
and (b) TASK-298's watchdog has a concrete file to tail. The watchdog *logic*
(parsing `permission_denials` / `is_error`) is TASK-298, not this PR.

### Diagram

```
  aida queue work SPEC --auto-complete --no-human[=MODE]
        │
        ▼  orchestrate() — six phases, unchanged
  Phase 1 implementer ──► spawns `aida queue work SPEC` ──► exec claude   (INTERACTIVE — STORY-276 will flip)
  Phase 2 CI
  Phase 3 reviewer    ──► spawns `aida queue work PR-N --no-human`
                              │
                              ▼  handle_queue_work(no_human=true)
                          exec claude -p "/aida-review --pr N"
                              --permission-mode bypassPermissions
                              --output-format stream-json --verbose
                              --session-id <uuid>      stdout ─► .aida/headless-logs/PR-N-<uuid>.jsonl
                              │
                              ▼  single turn: writes AIDA_REVIEW_VERDICT_FILE, exits 0
  Phase 4 merge ◄── orchestrator reads verdict file (existing handshake)
```

## Decisions

- **`--no-human` is a `queue work` flag, not orchestrator-only**. **Rationale**:
  the orchestrator drives the reviewer by spawning a plain `aida queue work
  PR-N` subprocess — that subprocess is where the headless launch actually
  happens, so the flag must be valid without `--auto-complete`. No clap
  `requires` constraint. The `[=MODE]` value (`reviewer-only` / `both`) is
  interpreted only on the orchestrator path; the plain path treats presence
  as a boolean "launch headless".
- **Bare `--no-human` = `both`, but `both` runs phase 1 interactively this
  cut**. **Rationale**: STORY-263 acceptance fixes bare = `both`. The headless
  implementer is STORY-276, so under `both` the orchestrator prints a one-line
  deferral note pointing at STORY-276 and runs phase 1 interactively. The flag
  grammar never changes when STORY-276 lands — it just wires phase 1.
- **`exec claude -p` keeps `exec` (not spawn)**. **Rationale**: the orchestrator
  already `.status()`-waits on the `aida queue work` subprocess; `claude -p`
  exits on its own when the single turn completes, so the wait returns. `exec`
  is unchanged plumbing.
- **Force `bypassPermissions` for headless**. **Rationale**: SPIKE-7 Q2 —
  `acceptEdits` is insufficient (Bash stays gated), `default` auto-denies
  silently. The resolved `permission_mode` is overridden for headless launches.
- **stream-json stdout → log file, not inherited**. **Rationale**: keeps the
  orchestrator's stdout clean (critical under `--json`) and hands TASK-298 a
  tail-able artifact. stderr stays inherited so Claude errors still surface.
- **No skill-template change**. **Rationale**: `/aida-review` step 7a already
  writes the verdict file and STOPs in orchestrator mode; the "press Ctrl+D"
  block is harmless printed text headless. The skill audit is TASK-297.

## Files (in build-order)

### `aida-cli/src/auto_complete.rs` — orchestrator mode type

- `enum NoHumanMode { ReviewerOnly, Both }`: new. `parse(&str)` accepts
  `""`/`both` → `Both`, `reviewer-only`/`reviewer` → `ReviewerOnly`, else err.
- `fn slug`: stable spelling for notes/telemetry.

### `aida-cli/src/session.rs` — headless launch

- `fn claude_headless_args(prompt, session_id) -> Vec<String>`: pure — builds
  the `-p … --permission-mode bypassPermissions --output-format stream-json
  --verbose --session-id` arg vector. Unit-tested.
- `fn exec_claude_headless(prompt, session_id, log_path) -> Result<()>`: sets
  `stdout` to the log file, execs `claude` with `claude_headless_args`.

### `aida-cli/src/cli.rs` — the flag

- `Work { … }`: add `no_human: Option<String>` —
  `#[clap(long, value_name = "MODE", num_args = 0..=1,
  default_missing_value = "both", aliases = ["unattended", "headless"])]`.

### `aida-cli/src/main.rs` — thread it through

- `QueueCommand::Work` arm: destructure `no_human`; pass to
  `handle_auto_complete*` and `handle_queue_work`.
- `fn handle_queue_work`: new `no_human: bool` param. Build the headless log
  path from `project_root`. At the `QueueWorkLaunch::Fresh` arm, call
  `exec_claude_headless` when `no_human`. Pre-flight summary gains a
  "launch: headless" line.
- `fn handle_auto_complete` / `fn run_auto_complete`: new
  `no_human: Option<NoHumanMode>` param.
- `struct RealPhaseDriver`: add `no_human: Option<NoHumanMode>` field.
- `RealPhaseDriver::run_reviewer`: append `--no-human` to the `aida queue work
  PR-N` subprocess args when `no_human` is set.
- `fn handle_auto_complete_batch` / `struct RealBatchDriver`: thread
  `no_human` through to `run_auto_complete`.
- Orchestrator entry: when `no_human == Some(Both)`, print the STORY-276
  deferral note.

### `docs/autonomous-drain.md` — new doc

- When `--no-human` is appropriate (small specs, clear acceptance) vs not.
- Reviewer-only cut today; implementer phase tracked as STORY-276.

### `CLAUDE.md` — one note

- `--no-human` under the batch-convention paragraph: interactive = better
  decisions, autonomous = better throughput.

## Critical Files

- `aida-cli/src/auto_complete.rs`
- `aida-cli/src/session.rs`
- `aida-cli/src/cli.rs`
- `aida-cli/src/main.rs`
- `docs/autonomous-drain.md`
- `CLAUDE.md`

## Reusable helpers (do not reimplement)

- `auto_complete::AutoCompleteVariant::parse` / `slug` — the pattern
  `NoHumanMode` mirrors exactly.
- `session::exec_claude_with_session` / `exec_claude` — the interactive launch
  path `exec_claude_headless` sits beside.
- `RealPhaseDriver::discover_orchestrated_lease`, `read_verdict_file` — the
  reviewer verdict handshake; unchanged.
- `find_main_worktree_root` — resolves the main clone for the log dir.

## Risks + gotchas

1. **Risk**: headless reviewer's stream-json corrupts the orchestrator's
   `--json` stdout. **Mitigation**: redirect Claude stdout to a log file;
   never inherit it.
2. **Risk**: the reviewer's worktree (and its `.aida/`) is deleted when the
   orchestrator ends the session — a log written there vanishes.
   **Mitigation**: log path is built from `find_main_worktree_root()`, not the
   session worktree.
3. **Risk**: `both` implies a headless implementer that does not exist yet —
   silent wrong behaviour. **Mitigation**: explicit one-line deferral note
   citing STORY-276; phase 1 demonstrably runs interactive.
4. **Risk**: a headless reviewer that bails writes no verdict file.
   **Mitigation**: existing `read_verdict_file` → `NoVerdict` failure stops
   phase 3 with the BUG-218 recovery hint. Full `permission_denials` detection
   is TASK-298.

## Tests (named, not "add tests")

- `no_human_mode_parse_accepts_all_forms` — `""`/`both`/`reviewer-only`, bad.
- `no_human_mode_slug_round_trips` — `parse(slug())` is identity.
- `claude_headless_args_has_spike7_mandatory_flags` — contains `-p`,
  `bypassPermissions`, `stream-json`, `--verbose`, `--session-id`.
- `claude_headless_args_never_uses_bare` — SPIKE-7 auth gotcha guard.

## Verification

```bash
cd /home/joe/ai/aida-epic-23
cargo build -p aida-cli 2>&1 | tail -3
cargo test -p aida-cli no_human 2>&1 | tail -8
cargo test -p aida-cli claude_headless 2>&1 | tail -8
# flag surface
./target/debug/aida queue work --help 2>&1 | grep -A2 no-human
./target/debug/aida queue work --unattended --help >/dev/null 2>&1 && echo "alias ok"
# negative: bad mode value rejected by the orchestrator
./target/debug/aida queue work FOO-1 --auto-complete --no-human=bogus 2>&1 | grep -qi 'unknown' && echo "bad-mode rejected"
```

## Followups

- STORY-276 — headless implementer phase (`--no-human=both` wires phase 1).
- TASK-297 — skill-template audit + `AIDA_NO_HUMAN` env var.
- TASK-298 — stream-json watchdog: parse `permission_denials` / `is_error`.

## Related

- STORY-263 — this spec.
- SPIKE-7 / `docs/spikes/2026-05-16-claude-headless.md` — empirical basis.
- STORY-246 — the `--auto-complete` orchestrator this extends.
- TASK-285 — `--batch --auto-complete`; `--no-human` composes with it.
