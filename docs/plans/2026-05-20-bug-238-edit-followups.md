# Plan: BUG-238 — `aida edit --status completed` triggers ## Followups parse

Date: 2026-05-20
Specs: BUG-238
Status: Done
Complexity: Small

## Approach

The plan `## Followups` auto-parse (TASK-96) was wired into two paths:

1. `aida queue done <ID>` — the implementer's "I'm done" path.
2. `aida pull` Done→Completed auto-bump (STORY-86) — promotes a Done spec to Completed when a referencing commit lands on `main`.

`aida edit <ID> --status completed` (and `--status done`) was **not** wired. The `/aida-review` skill's step 10 marks specs Completed directly via this path, so plan followups for every spec completed through the review skill were silently dropped (BUG-233's two followups had to be hand-filed; this BUG is the patch that closes the hole).

The fix hooks `extract_plan_followups` into `Command::Edit` after the successful save, gated on the new status being `Done` or `Completed`. The existing `FOLLOWUPS_MARKER` comment guards idempotency — whichever extraction path runs first wins, so a direct-edit completion followed by an `aida pull` auto-bump (or vice-versa) doesn't double-file. Interactivity follows the same TTY/non-TTY discipline as the rest of AIDA: TTY → per-bullet prompt; non-TTY (skill, CI, `</dev/null`) → file all (otherwise we'd reproduce the silent-loss this BUG fixes).

```
aida edit <ID> --status completed   ← /aida-review skill step 10
     ↓
Command::Edit handler (aida-cli/src/main.rs, the `Command::Edit { … }` arm)
     ↓ status canonicalized → "Done" | "Completed"?
     ↓ yes
extract_plan_followups(storage, project_root, spec_id, …, interactive=isatty)
     ↓
   FOLLOWUPS_MARKER comment already present? → skip (idempotent)
   else → parse plan ## Followups, file accepted bullets as child TASKs,
          stamp [aida:followups] marker comment
```

## Decisions

- **Trigger on both `Done` and `Completed`** — not just Completed. `aida edit --status done` is also a direct terminal-completed transition; the BUG-238 acceptance bullet calls this out explicitly. The shared helper handles both the same way.
- **TTY-based interactive gate** — `std::io::IsTerminal::is_terminal(&std::io::stdin())`. The `/aida-review` skill runs non-interactively, and that's exactly the silent-loss case we're fixing; defaulting to "file all" in non-TTY mode mirrors what the STORY-86 auto-bump already does (`interactive=false`). Interactive humans at a real terminal still get the per-bullet `[y/N/skip]` prompt.
- **Idempotency reuse, not re-invention** — `extract_plan_followups` already short-circuits when the `FOLLOWUPS_MARKER` comment exists. The new wiring inherits this guard for free, so the BUG-238 acceptance bullet "direct-completion then a later auto-bump does not double-file" is enforced without any new code.
- **Best-effort failure surface** — extraction errors print a `Warning:` and continue, matching the `queue done` and `aida pull` call sites. A followup-parse failure must never break the underlying `aida edit` (whose primary purpose is the status flip itself).

## Files (in build-order)

1. `aida-cli/src/main.rs` — the `Command::Edit` arm. Add the trigger block after the STORY-106 workflow-hint block, before the TASK-358 NeedsAttention cleanup block (same locality as the other post-save effects).
2. `tests/test_bug238_edit_followups.sh` — new integration shell test covering 5 cases (Completed triggers, Done triggers, idempotency, opt-out via `AIDA_AUTO_FOLLOWUPS=false`, non-terminal flip does not trigger).

## Critical Files

- `aida-cli/src/main.rs` — the wiring change is here.
- `tests/test_bug238_edit_followups.sh` — the regression test.

## Reusable helpers

The fix relies entirely on already-shipped TASK-96 primitives — no new helpers were added:

- `extract_plan_followups` (aida-cli/src/main.rs) — the shared followup-parse + child-task-filing routine.
- `FOLLOWUPS_MARKER` (aida-cli/src/main.rs) — the idempotency-guard comment prefix.
- `auto_followups_disabled` (aida-cli/src/main.rs) — the `AIDA_AUTO_FOLLOWUPS` opt-out check (inside `extract_plan_followups`).
- `find_project_root` (aida-cli/src/main.rs) — locates the project root for `docs/plans/` discovery.
- `std::io::IsTerminal::is_terminal` (stdlib) — TTY detection, already used elsewhere in `main.rs`.

## Risks + gotchas

- **A spec re-opened with `--force` then re-completed will skip the parse** — by design. The `FOLLOWUPS_MARKER` comment is the idempotency record; once stamped, the parse is done. If a user *intentionally* wants to re-parse after editing the plan file, they'd need to delete the marker comment first. This matches `queue done` + auto-bump behavior; no new edge.
- **A non-TTY caller now files-all by default** — this is the *correct* behavior for `/aida-review` (the motivating case) but it does mean a script piping `aida edit --status completed </dev/null` over many specs will file every plan followup without prompting. The `AIDA_AUTO_FOLLOWUPS=false` opt-out is the escape hatch (test 4 covers this).
- **Workspace tests load the heavy clap tree** — the existing `RUST_MIN_STACK = "8388608"` in `.cargo/config.toml` covers this; the new shell test exercises the built binary, not the test harness, so no clap stack interaction.

## Tests (named)

- `tests/test_bug238_edit_followups.sh::test_1_completed_triggers_parse` — `aida edit --status completed` on a fresh spec with a plan `## Followups` section → 2 child TASKs filed.
- `tests/test_bug238_edit_followups.sh::test_2_done_triggers_parse` — `aida edit --status done` → 1 child TASK filed.
- `tests/test_bug238_edit_followups.sh::test_3_idempotency` — re-open with `--force`, re-complete → child count stays at 2 (no double-filing).
- `tests/test_bug238_edit_followups.sh::test_4_opt_out` — `AIDA_AUTO_FOLLOWUPS=false aida edit --status completed` → 0 children filed.
- `tests/test_bug238_edit_followups.sh::test_5_non_terminal_flip` — `aida edit --status planned` → 0 children filed (parse only fires on terminal-completed transitions).

## Verification

```bash
# Build the debug binary and run the regression test suite.
cargo build -p aida-cli
bash tests/test_bug238_edit_followups.sh
# Expect: "=== All BUG-238 tests passed ==="

# Full aida-cli + aida-core test suites stay green.
cargo test -p aida-cli --quiet
cargo test -p aida-core --quiet

# Formatting and clippy stay clean (no NEW warnings — 219 pre-existing).
cargo fmt --all -- --check
```

## Followups

- None — the fix is self-contained. The two casualties of the original silent-loss (the AIDA_ZEN-provenance BUG and the run-UUID-into-drain-state TASK) were already hand-filed at BUG-238 origin time per the BUG description.

## Related

- TASK-96 — the `## Followups` auto-parse this BUG patches a hole in.
- STORY-86 — the Done→Completed auto-bump (the other path that already fired the parse).
- BUG-233 / PR-94 — the spec that surfaced the silent loss in real use.
