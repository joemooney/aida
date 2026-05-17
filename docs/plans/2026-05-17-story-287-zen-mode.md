# Plan: STORY-287 — `--zen` autonomy mode (three-mode taxonomy, slice 1)

Date: 2026-05-17
Specs: STORY-287
Status: In Progress
Complexity: ~90 prod LOC, ~50 test LOC, 1 commit, risk low

## Approach

STORY-287 introduces a three-mode autonomy ladder: **default** / `--zen` /
`--no-human`. Its `--no-human` punt behavior is load-bearing on STORY-276
(headless implementer) and STORY-285 (findings persistence), neither shipped.
Per the user's slicing comment on the spec, this slice ships **`--zen` only**
— pure prompt-classification + auto-resolve, implementable today — and defers
the `--no-human` punt artifact to a follow-up sub-TASK.

`--zen` = "advisor on standby": the human is at the keyboard but does not
want to click-yes through mechanical prompts. Under it, `kind:confirmation`
prompts auto-resolve to option 1; `kind:design-fork` prompts still pause.

The enforcement model is **convention, not runtime interception**. AIDA has
no hook on Claude Code's `AskUserQuestion` tool — prompts are agent-driven.
So `--zen` works by: (1) the CLI sets `AIDA_ZEN=1` in the launched session's
environment; (2) skill templates tag each prompt with a `kind:` annotation;
(3) the skill instructions tell the agent to auto-resolve `kind:confirmation`
prompts when `$AIDA_ZEN` is set.

Env-var propagation is the whole mechanism — no plumbing through
`handle_queue_work` or the orchestrator. `std::env::set_var("AIDA_ZEN","1")`
in the dispatch arm is inherited by every child: the direct `exec_claude`,
and the `--auto-complete` orchestrator's spawned `aida queue work` phase
subprocesses (which re-exec `claude` inheriting the var). Mirrors how
`AIDA_SESSION_ROLE` already propagates.

### Diagram

```
aida queue work --zen
  └─ dispatch: resolve_autonomy_mode(zen, no_human)
       ├─ NoHuman  → remove_var(AIDA_ZEN)          (--no-human wins, warn if both)
       ├─ Zen      → set_var(AIDA_ZEN=1)
       └─ Default  → (nothing)
            ↓ inherited by
       exec_claude / orchestrator subprocess → claude session
            ↓ skill reads $AIDA_ZEN
       kind:confirmation → auto-resolve option 1
       kind:design-fork  → surface (pause)
```

## Decisions

- **Convention over runtime interception.** `AskUserQuestion` is agent-driven;
  AIDA cannot intercept it. `--zen` is a behavioral contract carried by skill
  templates, enforced by the agent reading `$AIDA_ZEN`. Matches the STORY's
  "Out of scope: programmatic inference — skill author's job."
- **Env var IS the propagation.** `set_var`/`remove_var` once in the dispatch;
  every child process inherits it. No new parameter on `handle_queue_work` or
  the orchestrator functions. Same shape as `AIDA_SESSION_ROLE`.
- **Precedence at the CLI, not just in skills.** `resolve_autonomy_mode`
  encodes `--no-human` > `--zen` > default. When `--no-human` is effective the
  dispatch actively `remove_var`s `AIDA_ZEN` so a stale flag/env never reaches
  a headless session. Skills also state the precedence (belt and suspenders).
- **Annotation form:** `<!-- kind:confirmation -->` HTML comment directly
  above the prompt prose. STORY-named, survives rendering, lint-greppable.
- **Scope: 4 core skills** — `/aida-pickup`, `/aida-implement`, `/aida-pr`,
  `/aida-review`. Other skills are a STORY-named follow-up.
- **Lint deferred.** STORY lists the skill-template lint as "sub-TASK if
  needed"; filed as a follow-up rather than folded in (user's scope choice).

## Files (in build-order)

### `aida-cli/src/cli.rs` — `--zen` flag
Add `zen: bool` to the `Queue { Work { .. } }` variant. `///` doc comment
for `--help`; `trace:` marker on a plain `//` line (TASK-268).

### `aida-cli/src/main.rs` — precedence + propagation + pre-flight
- `AutonomyMode` enum + `resolve_autonomy_mode(zen, no_human)` pure helper,
  placed after `resolve_queue_work_permission_mode`.
- Dispatch arm: destructure `zen`; resolve mode; `set_var`/`remove_var`
  `AIDA_ZEN`; warn when both flags set.
- `handle_queue_work` pre-flight: an `autonomy: zen` line when `AIDA_ZEN` set.
- Tests: `autonomy_mode_*` in the existing test module.

### `aida-core/templates/skills/{aida-pickup,aida-implement,aida-pr,aida-review}.md`
Add an identical "Autonomy mode (`$AIDA_ZEN`)" section + `<!-- kind:* -->`
annotations above each prompt. Edit masters only; `make sync-templates`.

### `docs/aida-discipline/skill-prompt-kinds.md` — NEW
Skill-author convention doc: the two kinds, classification guidance,
option-1 convention, annotation form.

### `docs/autonomous-drain.md` — three-mode table
Add the default / `--zen` / `--no-human` table + "when to use each".

### Memory `feedback_three_mode_autonomy_taxonomy.md`
`metadata.propagation: scaffolding-pack`; the human-role-vs-pause-behavior rule.

## Critical Files

- `aida-cli/src/main.rs` — `handle_queue_work` (fn at ~43833), the `Queue {
  Work }` dispatch arm (~42060), `resolve_queue_work_permission_mode` (~46056).
- `aida-cli/src/cli.rs` — `Work` variant (~2400-2620).
- `aida-cli/src/session.rs` — `exec_claude` / `exec_claude_headless`: confirm
  no `.env_clear()`, so `AIDA_ZEN` inherits.
- `aida-core/templates/skills/aida-review.md` — existing `AIDA_HEADLESS`
  pattern (step 7b) to mirror for the `$AIDA_ZEN` check.

## Reusable helpers (do not reimplement)

- `AIDA_SESSION_ROLE` set-before-exec pattern (`handle_queue_work`) — the
  template for `AIDA_ZEN` propagation.
- `NoHumanMode` (`auto_complete.rs`) — the existing autonomy-flag parse;
  `resolve_autonomy_mode` composes with `no_human.is_some()`.
- `AIDA_HEADLESS` skill convention (`aida-review.md` step 7b) — the
  `echo "${AIDA_VAR:-}"` env-check idiom for `$AIDA_ZEN`.

## Risks + gotchas

- **Orchestrator env inheritance** — relies on `std::process::Command` not
  calling `.env_clear()`. Verified: `run_implementer`/`run_reviewer` and
  `exec_claude*` all inherit. A future `.env_clear()` would silently break
  `--zen` under `--auto-complete`. Note left in the dispatch comment.
- **`set_var` is process-global** — intended (children inherit) but means a
  later in-process code path sees `AIDA_ZEN`. Harmless: only skills read it.
- **Skill drift** — annotating prompts is manual; the deferred lint is what
  keeps new prompts annotated. Until it lands, "default kind = design-fork"
  (pause-safe) covers un-annotated prompts.

## Tests (named, not "add tests")

- `autonomy_mode_default_when_no_flags`
- `autonomy_mode_zen_flag_alone`
- `autonomy_mode_no_human_alone`
- `autonomy_mode_no_human_beats_zen` — both set → `NoHuman` (precedence)

## Verification

```bash
cargo build -p aida-cli
cargo test -p aida-cli autonomy_mode
aida queue work --help | grep -A2 -- --zen        # flag visible, no SPEC-ID leak
cargo fmt --all -- --check
make sync-templates                                # skill symlinks intact
aida plan verify docs/plans/2026-05-17-story-287-zen-mode.md
```

## Followups

- `--no-human` design-fork punt behavior — auto-resolve design-forks under
  `--no-human` + file a `kind:design-fork-punted` finding. Blocked on
  STORY-276 (headless implementer) + STORY-285 (findings persistence).
- Skill-template lint — warn when an `AskUserQuestion`-style prompt lacks a
  `kind:` annotation. STORY flags it "sub-TASK if needed".
- Propagate `kind:` annotations to skills beyond the core 4.
- JSONL logging of auto-resolve decisions (`auto-resolved: kind=X`) — depends
  on TASK-307 (tee headless output).

## Related

- STORY-287 (this spec) · TASK-306 (mode-flag clarity) · STORY-276 · STORY-285
- TASK-307 · STORY-278 (reviewer findings — sibling pattern)
- `feedback_pause_for_design_input.md` · `feedback_pushback_on_overengineering.md`
- `docs/autonomous-drain.md` · `docs/skills-convention.md`
