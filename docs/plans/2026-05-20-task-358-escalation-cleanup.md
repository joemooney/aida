# Plan: TASK-358 — clean up lingering worktrees from unresumed `--escalate-blocks` punts

Date: 2026-05-20
Specs: TASK-358 (followup of STORY-306)
Status: Shipped
Complexity: ~250 prod LOC, ~140 test LOC, 1 commit, risk low

## Approach

STORY-306 deliberately stopped ending the implementer's session on a punt
because the advisor tier may resume that exact `claude --session-id <…>` and
needs the worktree alive. Correct for the resume case; under
`--escalate-blocks` (the default escalation mode), a punt that the advisor
escalates and is never resumed leaves the worktree (lease + manifest +
activity log + git worktree) behind indefinitely. Over time `aida-*`
worktrees accumulate, one per never-resumed escalation, with no cleanup pass.

Two trigger paths share one safety gate. The gate is a new optional field on
the lease, `escalated_to_human: Option<DateTime<Utc>>`, stamped only when the
orchestrator's `EscalateMode::Blocks` arm fires. The `EscalateMode::Defaults`
resume path deliberately omits the stamp so its worktree survives the resume
the implementer is about to perform. Triggers:

1. **Auto-clean on triage out of Needs Attention.** When `aida edit --status`
   (or the punt-resolving alternatives `set_status_from_str` path) takes a
   spec NeedsAttention → {Approved, InProgress, Rejected}, the edit handler
   scans for any lease whose `scope` matches the spec AND carries the
   `escalated_to_human` marker, and mechanically removes it (lease file,
   manifest, activity log, `git worktree remove --force`). The marker is the
   load-bearing gate: an interactive user session on the same spec has no
   marker and is left alone, and an advisor-resume's worktree (no marker) is
   preserved.

2. **Explicit prune verb.** `aida session prune --escalations` is the
   recovery surface for cases the auto-clean didn't catch (older triages
   that pre-date the code, write errors). It finds leases with the marker
   set whose spec has since left NeedsAttention and offers to clean them up;
   leases whose spec is still NeedsAttention are surfaced as info but
   skipped (still awaiting human triage).

### Diagram

```
  Phase 1 punts ──► advisor escalates (EscalateMode::Blocks)
                        │
                        ├─► driver.mark_implementer_lease_escalated()
                        │        │
                        │        └─► lease.escalated_to_human = Some(now)
                        │
                        └─► finish_escalated (exit 0, drain advances)

  ... time passes, human reviews `aida findings list` ...

  aida edit TASK-X --status approved   (triage out of NeedsAttention)
        │
        └─► cleanup_escalated_leases_for_spec("TASK-X")
                 │
                 └─► for each lease where scope=="TASK-X" && escalated_to_human.is_some():
                          force_cleanup_lease(lease)   // worktree + lease + manifest
```

## Decisions

- **The marker is a per-lease boolean (timestamp), not derived from the punt
  ledger.** A boolean on the lease answers "is this lease's worktree safe to
  remove?" with one disk read — no ledger correlation, no spec-status
  lookup at write time. The Option<DateTime<Utc>> shape doubles as audit
  data for the explicit prune (which surfaces when each was stamped).
- **Cleanup is per-spec, not per-claude-session-id.** The auto-clean knows
  the spec (the edit target); the lease's `scope` is the spec id for any
  orchestrator-launched session. Matching by scope is simpler than threading
  the claude-session-id through, and an unrelated lease with the same scope
  but no marker is left alone (the marker is the load-bearing gate, not the
  scope match).
- **Per-driver trait method with a default no-op.** `PhaseDriver::mark_
  implementer_lease_escalated` has a default empty impl so test drivers
  without a real lease stay simple; `RealPhaseDriver` implements it against
  its stored `implementer_lease`. Mirrors the pattern STORY-306 used for
  `run_advisor` / `resume_implementer`.
- **`force_cleanup_lease` extracts `session_end`'s mechanical body, skipping
  the interactive parts.** No CI probe, no live-claude refusal, no
  dirty-tree refusal, no prompts — the marker is the safety gate that
  promises the lease is safe to remove. Mirrors session_end's order
  (aggregate activity → strip runtime symlinks → unlink lease/manifest/
  activity → `git worktree remove --force`) so failure modes are familiar.
- **The escalate-blocks path stamps the marker BEFORE `finish_escalated`.**
  Stamping first means a panic in the epilogue still leaves the marker so
  the explicit prune can later clean up; stamping after would risk the
  marker never being written.
- **Reviewer-merge-escalation is out of scope.** TASK-358 is the design-fork
  followup. A reviewer that escalates the merge decision is a different
  lingering case (its phase-3 session, not the implementer's) — file as
  needed.

## Files (in build-order)

### `aida-cli/src/main.rs`

- `struct SessionLease`: add `escalated_to_human: Option<DateTime<Utc>>`
  with `#[serde(default, skip_serializing_if = "Option::is_none")]` for
  backward compatibility — legacy leases deserialize with `None`.
- All 6 SessionLease construction sites: init `escalated_to_human: None`.
- `fn mark_lease_escalated_to_human(project_root, lease_id) -> Result<()>`:
  parse the lease TOML, stamp the timestamp, atomic write back. Mirrors
  `update_lease_branch`.
- `fn force_cleanup_lease(project_root, lease) -> bool`: mechanical
  worktree+lease+manifest+activity tear-down. Returns true on a clean
  git-worktree-remove; logs warnings on partial failure but never bails.
- `fn cleanup_escalated_leases_for_spec(project_root, spec_id)`: dispatch
  helper used by the edit hook. Iterates `list_leases`, calls
  `force_cleanup_lease` for any matching the spec + marker.
- `fn session_prune_escalations(dry_run, yes) -> Result<()>`: explicit
  prune verb. Bucket leases (marker set, spec out of NeedsAttention →
  eligible; marker set, spec still NeedsAttention → skipped with info).
- `fn session_prune`: add `escalations: bool` parameter, dispatch to
  `session_prune_escalations` when set.
- `Command::Edit` handler (the modern backend-driven path, ~line 3776):
  capture `left_needs_attention` when the status transition takes the spec
  out of NeedsAttention; after `backend.update_requirement` succeeds, call
  `cleanup_escalated_leases_for_spec` (best-effort, missing project root
  is a quiet no-op).
- `fn edit_requirement_cli` (the legacy storage path, ~line 6300): same
  capture + cleanup pattern.
- `SessionCommand::Prune` dispatch arm in `handle_session_command`: thread
  the new `escalations` field through.

### `aida-cli/src/auto_complete.rs`

- `trait PhaseDriver`: add `fn mark_implementer_lease_escalated(&mut self)`
  with a default no-op.
- `fn resolve_punt_via_advisor`, `EscalateMode::Blocks` arm: call
  `driver.mark_implementer_lease_escalated()` BEFORE `finish_escalated`.
  The Defaults arm deliberately does NOT call it.
- `MockPhaseDriver`: track `mark_escalated_calls: usize` and implement
  the method.

### `aida-cli/src/cli.rs`

- `SessionCommand::Prune`: add `escalations: bool` field with
  `#[clap(long, conflicts_with = "orphans")]` and a `// trace:TASK-358`
  marker (plain `//` so the trace ID doesn't bleed into `--help`).

### `RealPhaseDriver` impl block (`main.rs`)

- `fn mark_implementer_lease_escalated`: best-effort — look up
  `self.implementer_lease`, call `mark_lease_escalated_to_human`, on
  failure print a `Note:` line pointing at `aida session prune
  --escalations` as the recovery path. Never blocks the terminal
  `finish_escalated`.

### `docs/autonomous-drain.md`

- Replace the "robust cleanup is a followup" gotcha with the shipped
  description of the auto-clean trigger + the explicit prune verb.

## Critical Files

- `aida-cli/src/main.rs` — SessionLease, helpers, edit-hook wiring, prune verb
- `aida-cli/src/auto_complete.rs` — orchestrator trait + escalate-blocks call
- `aida-cli/src/cli.rs` — `--escalations` flag
- `docs/autonomous-drain.md` — gotcha → shipped behavior

## Reusable helpers (do not reimplement)

- `update_lease_branch` (`main.rs`) — the parse-mutate-atomic-write
  pattern `mark_lease_escalated_to_human` mirrors exactly.
- `session_end`'s body (`main.rs`) — the worktree+lease teardown
  mechanics `force_cleanup_lease` extracts the non-interactive subset of.
- `list_leases` / `lease_path` / `leases_dir` (`main.rs`) — lease file
  iteration + path resolution.
- `aggregate_session_activity_into_roles` / `session_activity_path` /
  `session_manifest::manifest_path` — companion-file paths preserved
  through the cleanup.
- `RealPhaseDriver::implementer_lease` (`main.rs`) — the lease id minted
  by `run_implementer`, the input to the orchestrator's stamp call.
- `forbidden_attention_transition` (`aida-core/src/models.rs`) —
  STORY-332's transition gate; we hook AFTER it succeeds, never weaken it.

## Risks + gotchas

1. **Risk**: an in-flight `--escalate-defaults` resume coincides with a
   manual `aida edit TASK-X --status …`, and our cleanup races the resume.
   **Mitigation**: structural — the resume path never sets the marker, so
   the cleanup gate fails closed. A user manually editing a Needs Attention
   spec while a resume is in flight would already be a foot-gun on STORY-306
   independent of TASK-358; nothing here amplifies it.
2. **Risk**: the explicit prune verb scans the storage to bucket leases;
   on a storage that won't open (a fresh init, a corrupt cache), it
   silently buckets every marker as "spec status unknown → eligible" and
   could over-prune. **Mitigation**: when storage fails to load, the
   matcher treats every status as "not NeedsAttention" → eligible, but the
   `--yes` opt-in is the gate. Default behaviour prints the candidate
   list + prompts. Worst case a user with no storage prunes some
   abandoned worktrees, which is the operationally desired outcome anyway.
3. **Gotcha**: `git worktree remove --force` walks back through
   `cwd.canonicalize()`; we run it from `project_root`, not from cwd, so a
   user inside the about-to-be-removed worktree doesn't trip git's
   "current worktree" refusal. (Same pattern session_end uses.)

## Tests (named, not "add tests")

`aida-cli/src/auto_complete.rs`:
- `orchestrate_punt_advisor_escalates_blocks_skips_phases_2_to_6` —
  extended: asserts `driver.mark_escalated_calls == 1`. The escalate-blocks
  path MUST stamp the lease.
- `orchestrate_punt_advisor_escalates_defaults_resumes_with_default` —
  extended: asserts `driver.mark_escalated_calls == 0`. The escalate-
  defaults resume path MUST NOT stamp (would nuke the resume's worktree).

`aida-cli/src/main.rs::task_358_escalation_cleanup_tests`:
- `mark_lease_escalated_to_human_stamps_marker` — round-trip: write a lease
  with `None`, call the helper, re-read, assert `Some`.
- `cleanup_skips_lease_without_marker` — the load-bearing safety property:
  an unmarked lease for the same spec is NOT removed.
- `cleanup_skips_escalated_lease_for_other_spec` — per-spec scope: a marked
  lease for a different spec is NOT touched by the per-spec cleanup.
- `legacy_lease_without_field_deserializes_with_none` — backward compat
  for pre-TASK-358 lease files on disk.

## Verification

```bash
# --- automated ---
cargo test -p aida-cli --bin aida task_358_escalation_cleanup
cargo test -p aida-cli --bin aida auto_complete::tests::orchestrate_punt_advisor
cargo fmt --all -- --check

# --- flag surface ---
aida session prune --help | grep escalations
```

Definition of done: `--escalate-blocks` punt that is never resumed stamps
the marker; `aida edit --status approved` on that spec triggers cleanup
of the worktree + lease + manifest; `aida session prune --escalations`
finds eligible leases and skips still-parked ones; `--escalate-defaults`
resume path preserves its worktree.

## Followups

- A reviewer-merge-escalation lingering-lease cleanup (parallel case, the
  reviewer phase's session, not the implementer's). File when it bites.
- Telemetry: count `escalated_to_human` markers that exit via auto-clean
  vs the explicit prune vs survive past N days — feeds the STORY-325
  analysis layer.

## Related

- Builds on: STORY-306 (advisor escalation tier — its "punted session
  persists" decision is what created this leak), STORY-276 (headless
  implementer — the source of the punt), STORY-332 (Needs Attention +
  punt ledger).
- See also: `docs/autonomous-drain.md`, `docs/plans/2026-05-19-story-306-advisor-escalation-tier.md`.
