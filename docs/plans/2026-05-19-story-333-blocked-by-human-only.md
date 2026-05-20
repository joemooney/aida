# Plan: STORY-333 — Typed `blocked-by` + `human-only` markers (un-pickable pre-pickup gate)

Date: 2026-05-19
Specs: STORY-333
Status: Draft
Complexity: ~600 prod LOC, ~400 test LOC, 1 PR, risk medium

<!--
  STORY-333 closes the prose-only gap: a spec that can't or shouldn't be auto-worked
  must be machine-visible so the orchestrator/queue skip it BEFORE spawning a doomed
  phase 1. Two flavours: blocked-by (cleared when blocker hits Completed) and
  human-only (permanent property of the spec). trace:STORY-333
-->

## Approach

Add `BlockedBy` / `Blocks` as **typed** `RelationshipType` variants (not `Custom`) with an inverse, and a `human_only: bool` field on `Requirement`. Centralize the pre-pickup gate as a single `pickability(req, store)` helper returning `Pickable | Blocked(reason)`. Wire that helper into every spec-pickup site — `resolve_queue_work_plan` head, `QueueCommand::Next`, `resolve_batch_members` (the batch-drain `next_head` source), and the cluster-mode entry resolver. `aida queue list` grows a **Blocked** section (sibling to "Done — awaiting merge") and a one-line ⚠ on any queued entry ordered ahead of its unsatisfied blocker. `aida queue add` and `aida queue move` print the same warning at placement time (never reject). `aida edit --human-only` / `--no-human-only` flips the field via the existing `Command::Edit` plumbing. `aida rel add … --type blocked-by` routes through the same matcher (with inverse `Blocks`). `aida doctor verify-relationships` already walks every `rel.target_id` against the uuid set, so dangling `blocked-by` composes for free — a confirming test, no new code. Live examples (STORY-276 `blocked-by` STORY-332, STORY-306 `blocked-by` STORY-276) wired at the end.

### Diagram

```
                                  ┌─────────────────────────────────┐
   queue work (head pickup) ───┐  │ pickability(req, store):        │
   queue next                  ├─▶│   - has unsatisfied blocked-by? │
   resolve_batch_members       │  │     → Blocked(UnsatisfiedBlocker│
   cluster resolve_entries  ───┘  │                  / Permanent)   │
                                  │   - human_only?                 │
                                  │     → Blocked(HumanOnly)        │
                                  │   - else Pickable               │
                                  └─────────────────────────────────┘
                                              │
                  ┌───────────────────────────┴──────────────┐
                  ▼                                          ▼
        Pickable → spawn phase 1                Blocked → skip + surface in
                                                queue list "Blocked" section.
                                                Auto-unblock when blocker
                                                reaches Completed (no manual step).
```

## Decisions

- **Decision 1**: Add `BlockedBy` and `Blocks` as **typed** `RelationshipType` variants (not `Custom("blocked-by")`). **Rationale**: typed gives a machine-readable inverse (`BlockedBy.inverse() == Some(Blocks)`), avoids string-comparison sprawl across the skip logic + `queue list` render + doctor check, and matches AC1's "reuse or add" framing — the existing typed variants (Parent/Child, Verifies/VerifiedBy) set the precedent for a directed dependency with a known inverse. `RelationshipType::from_str` already routes unknown strings to `Custom`, so a pre-existing `Custom("blocked-by")` (if anyone hand-rolled one) would *now* be parsed as the typed variant on next read — a clean upgrade, no migration.
- **Decision 2**: `human_only` is a `bool` field on `Requirement` with `#[serde(default, skip_serializing_if = "std::ops::Not::not")]`. **Rationale**: matches AC2 (typed field, not a tag), follows the same pattern as `archived: bool` on the same struct, omits cleanly from YAML when false so every existing spec round-trips unchanged.
- **Decision 3**: Centralize the skip logic in one `pickability(&Requirement, &RequirementsStore) -> Pickability` helper in `aida-core`. **Rationale**: four filter sites (`queue next`, `queue work` head, batch driver, cluster resolver) need identical semantics; duplicating the rule across four call sites is how subtle inconsistencies enter. One helper, one truth.
- **Decision 4**: A `blocked-by` edge whose target is `Rejected` surfaces as `Pickability::PermanentlyBlocked(target_spec)` — distinct enum variant from `UnsatisfiedBlocker`. **Rationale**: AC7 calls it out as a separate UX state; the `queue list` render must shout it (not silently skip) and the warning copy differs.
- **Decision 5**: `aida queue list`'s **Blocked** section renders below "Done — awaiting merge", sibling-style. **Rationale**: matches the existing TASK-222 pattern (in-flight section) and AC6's "shows a Blocked section — sibling".
- **Decision 6**: `queue add` / `queue move` placement warning is `eprintln!` to stderr, never bails. **Rationale**: AC8 is explicit ("warning, not error … operation still succeeds"). Staging work ahead of a blocker is sometimes deliberate (e.g. branching from the blocker's branch); refusing breaks legitimate workflow.
- **Decision 7**: Out of scope per spec: transitive cycle detection (`A blocked-by B blocked-by C`), auto-filing dangling blockers (TASK-311's job), soft "should come after" dependencies. **Rationale**: spec says so; tighter scope keeps the PR reviewable.
- **Decision 8**: `aida doctor verify-relationships` needs no new code path. **Rationale**: its existing scan walks every `rel.target_id` against `all_uuids` regardless of `rel_type`, so a dangling `BlockedBy` is caught by the same check. Add a confirming test and ship the AC.

## Files (in build-order)

Build-order matters: every commit must compile + test, so types land before consumers.

### `aida-core/src/models.rs` — relationship type + human_only field

- `enum RelationshipType`: add `BlockedBy` and `Blocks` variants.
- `impl fmt::Display for RelationshipType`: render `BlockedBy => "blocked-by"`, `Blocks => "blocks"`.
- `fn RelationshipType::from_str`: parse `"blocked-by" | "blocked_by" | "blockedby"` → `BlockedBy`; `"blocks"` → `Blocks`.
- `fn RelationshipType::inverse`: `BlockedBy → Some(Blocks)`, `Blocks → Some(BlockedBy)`.
- `fn RelationshipType::name`: canonical names (`"blocked_by"` / `"blocks"`).
- `struct Requirement`: add `pub human_only: bool` field with `#[serde(default, skip_serializing_if = "std::ops::Not::not")]`.
- `fn Requirement::new`: initialize `human_only: false`.

### `aida-core/src/pickability.rs` (new) — the centralized gate

- `enum Pickability { Pickable, Blocked(BlockedReason) }`
- `enum BlockedReason { UnsatisfiedBlocker(String /*spec_id*/), PermanentlyBlocked(String), HumanOnly }`
- `fn pickability(req: &Requirement, store: &RequirementsStore) -> Pickability` — walks `req.relationships` for `BlockedBy`, resolves target by `target_id` against the store, returns `PermanentlyBlocked` if target is Rejected, `UnsatisfiedBlocker` if target is not Completed, else falls through. Returns `HumanOnly` if `req.human_only`. Order: `HumanOnly` reported first if both apply (a human-only blocked spec is still human-only when the blocker clears).
- `fn pickability_reason_label(reason: &BlockedReason) -> String` — render helper for queue list (`"blocked-by STORY-332 (in progress)"`, `"human-only"`, `"blocked-by STORY-X (REJECTED — needs re-scoping)"`).

### `aida-core/src/lib.rs` — export `pickability`

- `pub mod pickability;` + re-export the enums.

### `aida-cli/src/cli.rs` — Edit flag, queue flags

- `Command::Edit`: add `--human-only` and `--no-human-only` flags (clap, mutually exclusive via a `clap::ArgGroup` or a single `Option<bool>`).

### `aida-cli/src/main.rs` — wire the gate into every consumer

- `Command::Edit` arm: thread `human_only` flag → `req.human_only = ...` with history entry recording the change.
- `Command::Rel(RelationshipCommand::Add { … })`: add `"blocked-by" | "blocked_by"` → `RelationshipType::BlockedBy`, `"blocks"` → `RelationshipType::Blocks` to the matcher (around current `"references"` line); bidirectional handler picks up `BlockedBy → Blocks` automatically via `inverse()`.
- `fn resolve_queue_work_plan` (head-pickup branch, `arg.is_none()` path): replace the inline `is_terminal_status` filter with `pickability(req, &store) == Pickability::Pickable`; skip un-pickable. Record the *skipped* specs so we can surface them in the kickoff banner ("ℹ skipped 2 un-pickable: STORY-276 (blocked-by STORY-332), TASK-2 (human-only)").
- `QueueCommand::Next` arm filter chain: same — replace inline status filter; carry skipped reasons for the existing stderr "skipped" hint.
- `fn resolve_batch_members`: after the existing `is_terminal_status` skip, add `pickability` skip with an eprintln noting which members were dropped.
- Cluster-mode entry resolver (`fn resolve_queue_work_plan` cluster branch around `let resolved_entries`): same filter + same banner.
- `QueueCommand::List` arm:
  - Build a `blocked: Vec<(QueueEntry, &Requirement, BlockedReason)>` alongside the existing in-flight build.
  - Filter the main queue render to exclude un-pickable; collect them for the new section.
  - For *queued* (pickable) items, compute "is this ahead of its blocker, which is *also* queued?" and stash a one-line ⚠ annotation per item.
  - Render the **Blocked** section (sibling to "Done — awaiting merge") with per-entry reasons and the appropriate emphasis for permanent blocks (red, not yellow).
- `QueueCommand::Add` arm: just before committing the entry, walk the queued entries and the new entry's `blocked-by` edges; if any `blocked-by` target is itself queued at a *later* position than the new entry's intended position, eprintln a `⚠ STORY-X queued ahead of STORY-Y, which blocks it` warning. Continue.
- `QueueCommand::Move` arm: same check — after computing the new position but before persisting, run the same inversion-detect.

### `aida-cli/src/auto_complete.rs` — batch drain composition

- No structural change needed: `BatchDriver::next_head` is the seam, and `resolve_batch_members` (its source) already filters. Add a doc comment noting un-pickable skip is centralized in `resolve_batch_members`.

### `aida-core/src/db/cache.rs` — cache schema

- Add `human_only INTEGER NOT NULL DEFAULT 0` to the requirements cache table; bump the schema version constant so existing caches auto-rebuild on next read (the standard cache-rebuild path runs when the version mismatches).
- Projection: write `req.human_only as i64` on insert. The blocked-by graph itself lives in `relationships` (already cached via the existing relationships table) — no new tracking needed.

### `aida-cli/src/main.rs` — `aida show` rendering polish

- Surface `human_only: true` in the spec card / detail view as a `[human-only]` chip near the type chip — otherwise the flag is invisible to a user inspecting the spec.

### `docs/plans/2026-05-19-story-333-blocked-by-human-only.md` — this plan

- Linked back from STORY-333 as a comment at end of implementation.

## Critical Files

At-a-glance blast radius.

- `aida-core/src/models.rs`
- `aida-core/src/pickability.rs` (new)
- `aida-core/src/lib.rs`
- `aida-core/src/db/cache.rs`
- `aida-cli/src/cli.rs`
- `aida-cli/src/main.rs`
- `aida-cli/src/auto_complete.rs`

## Reusable helpers (do not reimplement)

- `RelationshipType::from_str` / `inverse` / `name` (`aida-core/src/models.rs`) — already handles type round-tripping; the new variants slot in.
- `aida_core::is_terminal_status` (`aida-cli/src/main.rs`) — keep using it; pickability sits *alongside* it, not instead of it.
- `entry_matches_role_filter`, `entry_scope_session_match`, `resolve_queue_role_filter` — existing queue-filter primitives; pickability composes after them in the same `.filter(...)` chain.
- `record_role_activity` — existing edit/queue activity logger; new `--human-only` edits should record an `edit` event the same way other field changes do.
- `render_in_flight_grouped` (`aida-cli/src/main.rs:27780`) — the existing "Done — awaiting merge" section renderer is the template the Blocked section follows; the spacing, header, divider, dimmed hint pattern is identical.
- `Storage::queue_list` + `storage.load()` — every consumer site already has both; the pickability helper takes `&RequirementsStore` not `&Storage`, so call-site code patterns stay unchanged.
- `doctor_verify_relationships` (`aida-cli/src/main.rs:10192`) — already walks every rel target; the test just confirms a dangling `BlockedBy` falls into the same path.

## Risks + gotchas

1. **Risk**: A user's hand-rolled `Custom("blocked-by")` rel from before this STORY would be parsed as the new `BlockedBy` variant on next read — but a downstream consumer comparing on `RelationshipType::Custom(ref s) if s == "blocked-by"` would silently stop matching. **Mitigation**: grep verified no such consumer exists today (only `RelationshipType::Custom("sprint_assignment")` and `RelationshipType::Custom("sprint_contains")` are matched on string content). Add a test that round-trips `RelationshipType::from_str("blocked-by")` → typed variant → `Display` → `from_str` → same typed variant.
2. **Risk**: A blocked spec sitting at the queue head silently disappears from `aida queue next`/`aida queue work` with no signal — user thinks the queue is empty. **Mitigation**: head-pickup and `queue next` print a `ℹ` stderr line listing the skipped specs + reasons before falling through to the next pickable item. Visible-without-being-noisy is the bar.
3. **Risk**: Cache schema bump triggers a rebuild on every existing project's first read post-upgrade — could be slow on large stores. **Mitigation**: cache rebuild already exists for this exact reason and is logged with a one-line "(cache rebuilding…)" so it's not silent; we're following the pattern, not introducing it. Test coverage on a freshly-bumped cache rebuilding cleanly.
4. **Risk**: `pickability` reads from `&RequirementsStore`, so a blocker outside the loaded set (a future cross-store edge?) would look "unblocked" because the target_id wouldn't resolve. **Mitigation**: today the store is always loaded whole at every pickup site; cross-store edges aren't a thing. Document the assumption in the helper's doc comment; if/when cross-store comes (it's hypothetical), the helper signature changes deliberately.
5. **Risk**: `queue list`'s "ahead of blocker" check is O(N²) over the queue (each queued item × each of its blocked-by edges × position lookup). **Mitigation**: queue sizes are ~dozens, not thousands; the existing in-flight render and lease scan already iterate similar shapes. Benchmark only if a real complaint surfaces.
6. **Risk**: `human-only` on a Completed spec is meaningless but legal. **Mitigation**: pickability never even checks it (Completed isn't pickable on terminal-status grounds); the only oddity is the chip in `aida show`. Acceptable cosmetic.
7. **Risk**: The new `Blocked` section + `⚠` annotations bloat `queue list` for users without any blocked specs. **Mitigation**: both render conditionally — Blocked section only when non-empty; ⚠ only on actually-inverted entries. Empty-state cost is zero.

## Tests (named, not "add tests")

### `aida-core/tests/pickability_tests.rs` (new file)

- `pickability_pickable_when_no_blockers_no_human_only` — base case.
- `pickability_blocked_by_in_progress_target_reports_unsatisfied` — happy negative.
- `pickability_unblocks_when_blocker_reaches_completed` — auto-unblock.
- `pickability_blocked_by_rejected_target_reports_permanent` — permanent block surfacing.
- `pickability_human_only_reports_human_only` — flag-only case.
- `pickability_human_only_takes_precedence_over_blocked` — ordering invariant.
- `pickability_dangling_blocked_by_target_unknown_treated_as_blocked` — defensive: dangling edge doesn't accidentally unblock.

### `aida-core/src/models.rs` (inline `#[cfg(test)]` block)

- `relationship_type_blocked_by_round_trips_through_string` — `from_str` + `Display` symmetry.
- `relationship_type_blocked_by_inverse_is_blocks_and_back` — inverse machinery.
- `requirement_default_human_only_is_false_and_serializes_absent` — yaml round-trip with `skip_serializing_if`.

### `aida-cli/src/main.rs` (inline `#[cfg(test)]` blocks where queue logic already has them)

- `queue_next_skips_blocked_picks_next_pickable` — integration.
- `queue_next_skips_human_only_picks_next_pickable`
- `queue_work_head_pickup_skips_unpickable_and_surfaces_reasons` — banner + advance.
- `resolve_batch_members_skips_unpickable_members` — drain composition.
- `queue_list_renders_blocked_section_with_reasons`
- `queue_list_annotates_queued_ahead_of_blocker_with_warn`
- `queue_add_inverting_blocked_by_emits_warning_and_succeeds`
- `queue_move_inverting_blocked_by_emits_warning_and_succeeds`

### `aida-cli/src/main.rs` — doctor coverage

- `doctor_verify_relationships_catches_dangling_blocked_by` — confirms the composition works; no new code path, just a test.

## Verification

End-to-end bash smoke. Drives every AC from a fresh repo.

```bash
TMP=$(mktemp -d); cd "$TMP" && git init -q && aida init -q

# Two specs, B blocks A
aida add --title "A: depends on B" --type story --status approved --id-out a >/dev/null
aida add --title "B: blocker" --type story --status approved --id-out b >/dev/null
A=$(aida list --status approved --format json | jq -r '.[] | select(.title|startswith("A:")) | .spec_id')
B=$(aida list --status approved --format json | jq -r '.[] | select(.title|startswith("B:")) | .spec_id')

# AC1 + AC2: typed blocked-by rel; human-only flag
aida rel add "$A" "$B" --type blocked-by
aida add --title "H: human gate" --type task --status approved
H=$(aida list --status approved --format json | jq -r '.[] | select(.title|startswith("H:")) | .spec_id')
aida edit "$H" --human-only

# Queue all three; A is ahead of B (inversion)
aida queue add "$A" --for implementer
aida queue add "$B" --for implementer
aida queue add "$H" --for implementer
# Expect: warning "A queued ahead of B" emitted on $A add (AC8)

# AC3 + AC6 + AC9: head-pickup skips A (blocked) and H (human-only), picks B
AIDA_SESSION_ROLE=implementer aida queue next | tee /tmp/next.out
grep -q "$B" /tmp/next.out || { echo "FAIL: queue next did not pick B"; exit 1; }
aida queue list | grep -q "Blocked" || { echo "FAIL: no Blocked section"; exit 1; }
aida queue list | grep -E "$A.*ahead of.*$B" || { echo "FAIL: missing ahead-of-blocker ⚠"; exit 1; }

# AC4: batch drain skips un-pickable. Tag a batch with all three, drain.
aida edit "$A" --tags "batch:t1"
aida edit "$B" --tags "batch:t1"
aida edit "$H" --tags "batch:t1"
# Dry-run to validate skip ordering without launching claude
aida queue work --batch t1 --dry-run | grep -q "$B" || { echo "FAIL: batch picks not B"; exit 1; }

# AC5: ship B (mark Completed); A becomes pickable
aida edit "$B" --status completed --force
AIDA_SESSION_ROLE=implementer aida queue next | grep -q "$A" || { echo "FAIL: A still blocked after B Completed"; exit 1; }

# AC7: rejected blocker → permanent block
aida add --title "X: depends on Y-rejected" --type story --status approved
X=$(aida list --status approved --format json | jq -r '.[] | select(.title|startswith("X:")) | .spec_id')
aida add --title "Y: rejected" --type story --status approved
Y=$(aida list --status approved --format json | jq -r '.[] | select(.title|startswith("Y:")) | .spec_id')
aida rel add "$X" "$Y" --type blocked-by
aida edit "$Y" --status rejected --force
aida queue add "$X" --for implementer
aida queue list | grep -E "$X.*REJECTED|$X.*permanent" || { echo "FAIL: permanent-block not surfaced"; exit 1; }

# AC12: doctor catches dangling blocked-by
# Hand-edit the YAML to break the target_id, then run doctor — must report it
# (left as a unit test rather than smoke because YAML hand-edit is fragile)

echo "ALL ACS GREEN"
```

## Followups

- Convert remaining prose "(requires X)" / "blocked on X" comments across the spec base into typed `blocked-by` edges.
- `aida queue list --tree` annotation for `blocked-by` edges in the tree view.
- Statusline indicator when a session is on a `human-only` spec (shouldn't happen — but cheap defensive UX).
- `aida punt --category blocked-dependency` could auto-offer to file the `blocked-by` rel as part of the punt flow (closes the PuntCategory::BlockedDependency loop noted in `aida-core/src/models.rs:62`).
- Soft "should come after" dependencies as a separate typed rel — out-of-scope per AC, but the obvious next layer.

## Related

- Builds on: the existing typed-relationship machinery (`RelationshipType` + `aida rel add`)
- Composes with: BUG-241 (orchestrator outcome model — un-pickable is a *pre-pickup* state, BUG-241 is the *post-phase* safety net)
- Composes with: STORY-332 (`/aida-punt` + `PuntCategory::BlockedDependency`) — punt creates the signal, this STORY consumes it
- Live examples: STORY-276 `blocked-by` STORY-332; STORY-306 `blocked-by` STORY-276 (wired at end of implementation)
- See also: TASK-311 (dialog-role spec audit — typed `blocked-by` makes the dangling-dependency lint machine-checkable)
- See also: TASK-310 (`--batches A,B,C` chaining — must compose with the new skip)
