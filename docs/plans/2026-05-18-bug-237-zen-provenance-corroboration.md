# BUG-237 — AIDA_ZEN provenance corroboration

- **Date:** 2026-05-18
- **Specs:** BUG-237
- **Status:** In progress
- **Complexity:** Medium

## Approach

`AIDA_ZEN=1` enables zen mode (auto-resolve `kind:confirmation` prompts,
including the merge confirmation). Today it is trusted on sight — a leaked /
stale `AIDA_ZEN=1` inherited into an interactive shell silently enables zen and
can auto-merge a PR the user never authorized.

Mirror BUG-233's `AIDA_AUTO_COMPLETE` corroboration. `AIDA_ZEN` has two
legitimate origins, so it needs two corroboration paths:

1. **Orchestrator-set** (`aida queue work --auto-complete --zen`) — corroborate
   against the live orchestrator run marker (reuse BUG-233's run-UUID + marker
   file). The marker gains a `zen` field recording whether the run was `--zen`.
2. **Standalone** (`aida queue work --zen`) — corroborate against a per-session
   *zen-intent token* recorded in the session's lease.

A new `aida zen status` command (mirror of `aida orchestrator status`) prints
`zen` only when corroborated, else `interactive` + an informational note.
Skills branch off that word instead of reading the bare `$AIDA_ZEN` env var.

```
                 aida zen status
                       │
        AIDA_ZEN==1 ? ──┴── no ─→ interactive
              │ yes
   live orchestrator run? ──→ marker.zen ? zen(orchestrator) : uncorroborated
              │ no
   own lease has zen_intent_token? ──→ yes: zen(session-lease)
              │ no
        uncorroborated → interactive + note ("unset AIDA_ZEN" hint)
```

## Decisions

- **`AIDA_ZEN_TOKEN` is the provenance anchor.** The `--zen` dispatch arm mints
  a per-invocation UUID into `AIDA_ZEN_TOKEN`; the `Default`/`NoHuman` arms
  scrub it. So `AIDA_ZEN_TOKEN` is present iff *this* `aida queue work` was
  genuinely `--zen` — a leaked one is wiped at the dispatch door. The session
  lease copies it (standalone path); `RunMarkerGuard::register` reads its
  presence (orchestrator path).
- **Two paths, not one.** The orchestrator path can't use the lease (phase
  children's leases aren't `--zen`-flagged; the run is one zen decision). The
  standalone path can't use a run marker (no orchestrator). Each gets the
  mechanism that fits.
- **`AIDA_ZEN` itself is not scrubbed** in the `Default` arm — orchestrated
  phase children legitimately inherit it. Corroboration, not scrubbing, is the
  safety net; the leak is neutralized at `aida zen status`, not at the door.
- **Lease lookup is the unleakable anchor** for the standalone path: the lease
  is found by worktree-path-contains-cwd, which a stray env var cannot forge.

## Files (build order)

1. `aida-cli/src/orchestrator.rs` — `RunMarker.zen` field; `register` takes
   `zen: bool`; new `live_run_marker()`.
2. `aida-cli/src/zen.rs` *(new)* — `ZenContext`, pure `classify`, `detect`,
   verdict helpers. Mirrors `orchestrator.rs`.
3. `aida-cli/src/cli.rs` — `ZenCommand` enum + hidden `Command::Zen`.
4. `aida-cli/src/main.rs` — `mod zen`; `SessionLease.zen_intent_token`;
   dispatch mint/scrub of `AIDA_ZEN_TOKEN`; lease-creation reads it;
   `register` call passes `zen`; `Command::Zen` dispatch + `handle_zen_command`.
5. `aida-core/templates/skills/{aida-implement,aida-pr,aida-pickup,aida-review}.md`
   — autonomy section checks `aida zen status` instead of `echo "$AIDA_ZEN"`.

## Critical Files

- `aida-cli/src/orchestrator.rs` — the corroboration template being mirrored.
- `aida-cli/src/zen.rs` — the new corroboration module.
- `aida-cli/src/main.rs` — dispatch arm (`resolve_autonomy_mode` match), lease
  construction in `session_start`, `RunMarkerGuard::register` call site.

## Reusable helpers

- `orchestrator::{RunMarker, marker_path, is_valid_token, run_is_live}` —
  BUG-233's marker plumbing; `live_run_marker` joins it.
- `active_lease_for_cwd` / `SessionLease` (main.rs) — the standalone anchor.
- `process_probe::pid_is_alive` — liveness probe.

## Risks + gotchas

- The `AutonomyMode::Default` arm must scrub `AIDA_ZEN_TOKEN` or a leaked token
  re-creates the bug one layer down.
- Orchestrated phase children run `aida queue work` *without* `--zen` (env
  inheritance only) — their `Default` dispatch scrubs `AIDA_ZEN_TOKEN`, so they
  rely on the run-marker path, never the lease path. Intentional.

## Tests (named)

- `zen::tests::classify_*` — zen-off, orchestrator-zen, orchestrator-not-zen,
  standalone lease token, leaked-no-provenance.
- `zen::tests::uncorroborated_status_word_is_interactive` — the merge path
  keys off `status_word()`; assert it is never `zen` uncorroborated.
- `orchestrator::tests::run_marker_zen_round_trips`.

## Verification

```
cargo test -p aida-cli zen::
cargo test -p aida-cli orchestrator::
cargo build -p aida-cli
```

## Followups

- Audit `AIDA_EXIT_SENTINEL` / `AIDA_REVIEW_VERDICT_FILE` for explicit
  provenance corroboration — both are UUID-scoped file paths, so a leak is
  low-risk (stale path, nothing watches it), but neither is corroborated.
- Update `docs/autonomous-drain.md` zen-mode section to mention `aida zen
  status` as the corroborated check.

## Related

- BUG-233 (the `AIDA_AUTO_COMPLETE` corroboration this mirrors)
- TASK-327 (`AIDA_ZEN=0` parsing — sibling, different facet)
- STORY-287 (`--zen` mode)
