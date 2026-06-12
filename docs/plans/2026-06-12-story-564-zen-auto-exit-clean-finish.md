# STORY-564 — `aida queue work --zen`: auto-exit on a clean finish

- **Date:** 2026-06-12
- **Specs:** STORY-564 (subsumes BUG-500); follow-up TASK-758 (presence bias, acceptance #5)
- **Status:** Implemented (keyboard-supervised build)
- **Complexity:** Medium

## Approach

`--zen` (no orchestrator) always opened the PR then **paused** at the
grab-next/stop checkpoint — so even a clean finish with no human in the loop
forced a manual `stop` + `aida session end` (the BUG-500 friction). STORY-564
makes `--zen` auto-decide its exit by whether a human was *actually* needed.

The key realization: the `--zen` finish checkpoint is **not** a launcher
decision (the launcher already exited) — it happens *inside* the Claude session
at the `/aida-pr` finish step, which is skill-driven. So the fix is a
**substrate gate** the skill consults, not launcher control-flow.

```
--zen session runs ──► /aida-pr finish ──► `aida zen finish`
                                              │
              ┌───────────────────────────────┴───────────────┐
        auto-exit (clean)                                 pause (human-needed)
   skill runs `aida session end`                    render grab-next/stop table
        + exits, no round-trip                         (pre-STORY-564 behavior)
```

The clean-vs-human-needed signal has three inputs, OR'd toward pause:

1. **needs-human marker** — the session writes one (`aida zen needs-human
   --reason`) the instant it pauses on a `kind:design-fork` for the standby
   advisor or raises a punt. A session that only auto-resolves
   `kind:confirmation` prompts never marks itself.
2. **open punt for the spec** — read directly from `.aida/punts.jsonl`, scoped
   to the lease's spec + start time. Belt-and-suspenders: the gate sees punts
   even if the marker is missed.
3. **`--pause-always` / `[zen] auto_exit = false`** — the operator electing to
   drive grab-next by hand.

None of the three ⇒ `auto-exit`.

## Decisions

- **Signal source = session-marks + ledger** (operator pick): gate defaults to
  auto-exit; the session affirmatively marks human-involvement. Marker is a real
  on-disk substrate record, so the skill can't talk past the gate at finish.
- **Config policy + defer presence** (operator pick): `[zen] auto_exit` (default
  `true`) + `--pause-always` per-invocation override; acceptance #5 (presence
  bias) deferred to TASK-758 since presence is plumbed-not-wired today.
- **Marker keyed by lease id**, stored under the **main** worktree's
  `.aida/zen-needs-human/<id>.marker` — writer and reader both resolve
  `find_main_worktree_root()`, so they agree across sibling worktrees. Auto
  gitignored by the deny-by-default `.aida/*` rule.
- **`--pause-always` propagates via `AIDA_ZEN_PAUSE_ALWAYS=1`** (not the lease):
  a leak only ever *adds* a pause (the safe direction), so it needs no
  corroboration token (unlike `AIDA_ZEN`).

## Files (build order)

1. `aida-cli/src/zen.rs` — `ZenFinish`/`FinishPause` enums, pure
   `classify_finish`, marker helpers (`mark_needs_human`,
   `has_needs_human_marker`, `clear_needs_human_marker`), `ZEN_PAUSE_ALWAYS_ENV`.
   + 7 unit tests.
2. `aida-cli/src/cli.rs` — `ZenCommand::Finish` / `ZenCommand::NeedsHuman`;
   `--pause-always` on `QueueCommand::Work`. (SPEC-IDs kept out of `///` →
   `--help`, per TASK-268.)
3. `aida-cli/src/main.rs` — `read_zen_auto_exit` / `zen_pause_always_in_force`
   config helpers; `AIDA_ZEN_PAUSE_ALWAYS` propagation + no-op warning in the
   autonomy dispatch; `handle_zen_command` Finish/NeedsHuman arms;
   `clear_needs_human_marker` call in `session_end`.
4. `aida-core/templates/skills/aida-pr/SKILL.md` — finish checkpoint consults
   `aida zen finish`; auto-exit runs the shared session-end cleanup.
5. `aida-core/templates/skills/aida-implement.md` — instruct the session to run
   `aida zen needs-human` when it surfaces a `kind:design-fork` / punts.

## Critical files

- `aida-cli/src/zen.rs::classify_finish` — the pure decision; unit-tested.
- `aida-cli/src/main.rs::handle_zen_command` (`Finish` arm) — assembles the
  three signals from the live lease + ledger + env/config.

## Reusable helpers

- `zen::detect` (BUG-237) — corroborated zen verdict; reused for `is_zen`.
- `punt::read_ledger` — punt records; filtered by spec + `started_at`.
- `active_lease_for_cwd` — resolves the session lease (id + scope + started_at).
- `read_archive_auto_after_days` — pattern mirrored by `read_zen_auto_exit`.

## Risks + gotchas

- Both failure modes are mild: wrong-pause = manual cleanup (status quo);
  wrong-auto-exit = operator re-runs `aida queue work`. Marker + ledger keep the
  common clean case (the BUG-500 target) fully automatic.
- Marker lives under the *main* root, not the per-spec worktree — verified
  writer/reader agree via `find_main_worktree_root()`.

## Tests

- `zen::tests::finish_*` (6) — clean→auto-exit, non-zen→pause, marker/punt/
  pause-always→pause, precedence.
- `zen::tests::needs_human_marker_roundtrips` — write/read/clear, per-session
  scoping, reason body.
- Regression: `session_end` (42) + `punt` (40) suites green.

## Verification (executed)

```bash
cargo test -p aida-cli zen::          # 16 passed
cargo fmt --all -- --check            # clean
cargo clippy -p aida-cli              # no new / no correctness warnings
# live dogfood (this very session is a corroborated --zen session on STORY-564):
aida zen finish --json                # {"decision":"auto-exit","reason":"clean",...}
aida zen needs-human --reason "..."   # ✓
aida zen finish --json                # {"decision":"pause","reason":"needs-human",...}
git check-ignore .aida/zen-needs-human/<id>.marker   # IGNORED
```

## Followups

- TASK-758 — presence bias (acceptance #5), wired when STORY-561 consumers land.

## Related

- BUG-500 (subsumed — cleanup-on-stop is acceptance #1), BUG-237 (zen
  corroboration), STORY-287 (three-mode autonomy), STORY-332 (punt ledger),
  STORY-561 (presence).
