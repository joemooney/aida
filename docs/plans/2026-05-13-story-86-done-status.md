# Plan: STORY-86 — Add `Done` status between `InProgress` and `Completed`

Date: 2026-05-13
Specs: STORY-86, EPIC-21, STORY-81, TASK-43, BUG-65
Status: Completed
Complexity: ~250 prod LOC, ~150 test LOC, 4 commits, risk medium-low

<!--
  Worked example for the structured plan template (TASK-92).
  Originally generated 2026-05-13 via /ultraplan, retrofitted 2026-05-14
  to match docs/plans/_TEMPLATE.md.
-->

Implemented across 4 commits on `epic-21-2` (2026-05-13):

- `de3cc1d5` — enum variant + serde wiring
- `a6f25289` — queue done flip + history/manifest
- `3b0ff9df` — auto-bump helper + pull/sync wiring + 8 new tests
- `224950b8` — React UI + skill templates + CLAUDE.md + META prompts

Verification: full workspace test suite (231 + 278) passes; release binary
built and `--status done` filter works against the live store. End-to-end:
STORY-86 itself flipped to `Done` via `aida queue done` — auto-bumped to
`Completed` once `aida pull` ran on main after PR merge.

## Approach

`aida history`, `aida show`, and the kanban board surface `Completed` for
two semantically different states: (1) **Done on a branch** — an agent ran
`aida queue done` but the PR hasn't merged yet; (2) **Merged to main** —
actually shipped. STORY-86 splits them: `aida queue done` flips to a new
`Done` status, and the existing post-pull pipeline auto-bumps `Done →
Completed` when a referencing commit lands on the default branch.
`Completed` keeps its existing semantics ("shipped on main"), so existing
data needs no migration. Implementation is contained: one new enum
variant, one new helper `auto_bump_done_to_completed` reused by `aida
pull` and `aida db sync --pull`, and a small set of display/serde
call-site updates.

### Diagram

```
            +---- aida queue done -----+
            |                          v
   InProgress ─────────────────────► Done ──── pull from main ────► Completed
                                      ^         (auto-bump scan)
                                      |
                       aida edit --status done  (manual)
```

```
        ┌── handle_pull_command ──┐
        │                         │
  git pull --ff-only (code) ──┐   │
                              │   │
  if on default branch:       │   │
    pre_sha = HEAD~before     │   │
    git log pre..HEAD ─► subjects │
                                  │
    for each REQ-ID in subject ───┤
      if status == Done           │
        flip → Completed          │
        stamp completed_at        │
        stamp completion_sha      │
        record_role_activity      │
                                  │
  git_ops::pull_rebase (store) ───┘
```

## Decisions

- **Where the auto-bump scan lives**: **Both** `handle_pull_command` (`aida
  pull`) AND `Command::Db(DbCommand::Sync { pull, .. })`. Same helper, two
  callsites. **Rationale**: users who scripted around `aida db sync --pull`
  keep working; the helper itself looks at the **code repo's** default
  branch in both callsites — neither needs a code-vs-store branch.
- **No separate `aida db reconcile-status` subcommand for v1**.
  **Rationale**: not required to ship; can wrap the helper as a hidden
  subcommand later if manual replay is needed (filed as followup).
- **No `#[serde(other)]` fallback for unknown status variants**.
  **Rationale**: would silently mis-route Done into a default and break
  the auto-bump invariant. Workspace tool, not a published library — older
  binaries failing to deserialize is acceptable.
- **`done` is non-terminal in `is_terminal_status_str`**. **Rationale**:
  `Done` can advance to `Completed`. Today the function returns true for
  `"done"` (treats agent-friendly synonym as Completed); that branch goes
  away. `is_terminal_status` (the enum form) keeps `Completed | Rejected`
  only, no addition.
- **Default-branch detection prefers `git symbolic-ref refs/remotes/origin/HEAD`**
  with `main`/`master` fallback. **Rationale**: works for cloned repos;
  fallback handles `aida init` projects without a remote. If neither
  matches, skip auto-bump silently — no remote means no merges to react to.

## Files (in build-order)

Order matters — top-to-bottom so each commit builds.

### `aida-core/src/models.rs` — enum + setters

- `enum RequirementStatus`: add `Done` variant between `InProgress` and `Completed`.
- `impl Display for RequirementStatus`: add `RequirementStatus::Done => write!(f, "Done")`.
- `fn set_status_from_str`: add `"done" => self.status = RequirementStatus::Done; self.custom_status = None;`. Note: `"done"` previously fell through to `custom_status` — verify no test relies on that.
- `struct ImplementationInfo`: add two optional fields, both `#[serde(default, skip_serializing_if = "Option::is_none")]`:
  - `completed_at: Option<DateTime<Utc>>` — set when auto-bump fires.
  - `completion_sha: Option<String>` — git SHA on the default branch that triggered the bump.
  Update `Default` + `ImplementationInfo::new` accordingly.

### `aida-cli/src/main.rs` — validation, terminal checks, queue done, colors, auto-bump

- `fn validate_status_input`: add `"done" => Ok("Done")`. Remove the `"done" => Ok("Completed")` alias — `done` is now its own state. Update the error message's value list.
- `fn is_terminal_status_str`: **do not** add `Done` — non-terminal. Today returns true for `"done"`; remove that branch. The history.rs twin needs the same removal.
- `fn is_terminal_status`: unchanged — keep `Completed | Rejected` only.
- `QueueCommand::Done` handler: change `r.set_status_from_str("Completed")` to `r.set_status_from_str("Done")`. Update prompt + success messages. Keep `implementation_info.implemented = true` stamping; leave `completed_at` / `completion_sha` unset (auto-bump sets them).
- `fn update_manifest_for_status`: add the `"done"` branch so it calls `session_manifest::mark_completed`. Keep `"completed"` and `"rejected"` mapping there too.
- Queue list color renderer (two sites): add `"Done" => status.bold().green()` for visual distinction from `Completed` (plain green).
- `update_manifest_for_status` callsite from queue done: change to `update_manifest_for_status(spec_id, "Done")`.
- `fn handle_pull_command`:
  - Snapshot `let pre_code_sha = git_ops::head_sha(&project_root).ok();` **before** the `git pull --ff-only` call.
  - After a successful code pull, call new helper `auto_bump_done_to_completed(&project_root, store_path, pre_code_sha.as_deref(), storage)`.
  - Render summary inline ("auto-bumped N Done → Completed" + first 5 spec_ids).
  - Skip when not on default branch (helper handles this).
- `Command::Db(DbCommand::Sync { pull, .. })`: same hook after `pull_rebase` succeeds. The auto-bump scan looks at the **code repo's** default branch, not the orphan branch.
- New helper `fn auto_bump_done_to_completed` (place near `print_pull_summary`):
  ```rust
  fn auto_bump_done_to_completed(
      project_root: &Path,
      store_path: &Path,
      pre_sha: Option<&str>,   // None ⇒ scan HEAD only (first pull)
      storage: &Storage,
  ) -> Result<Vec<(String, String)>> { ... }
  ```
  Steps: detect default branch (symbolic-ref or main/master fallback) → if current ≠ default, return empty. Build commit range `pre_sha..HEAD` (or `HEAD~50..HEAD` cap on first pull). `git -C project_root log --pretty=format:%H%x09%s%x09%(trailers:key=Aida-Store,valueonly,separator=%x00) range`. For each row: `extract_spec_ids_from_commit(subject)`; first commit wins. Load store; only flip if current status is exactly `RequirementStatus::Done` (idempotent). Inside `storage.update_atomically`: set status = Completed, stamp `completed_at` + `completion_sha`. Don't overwrite `implemented_at` / `implemented_by`. After write, call `record_role_activity(spec_id, "auto-completed")`. Return `Vec<(spec_id, commit_sha)>`.
- `AIDA_AUTO_BUMP` env opt-out, mirroring `auto_merge_gate_enabled`. Default on; respect `false|0|no|off`.

### `aida-cli/src/history.rs` — colorize + terminal check

- `fn colorize_status`: add `"Done" => status.bold().green().to_string()`. Keep `"Completed" => status.green()` plain.
- `fn is_terminal_status_str`: remove the `t.eq_ignore_ascii_case("done")` clause.
- `--all` flag's user-facing text stays correct.

### `aida-core/src/import.rs` — explicit parser table

- `fn parse_requirement_status`: add `"Done" => Some(RequirementStatus::Done)` and the missing `"Planned"` and `"In Progress"`/`"InProgress"` rows.

### `aida-core/src/db/sqlite_backend.rs` + `aida-core/src/db/postgres_backend.rs`

- `fn status_to_str`: add `RequirementStatus::Done => "Done"`.
- `fn str_to_status`: add `"Done" => RequirementStatus::Done`.
- Apply parallel two-line change in `postgres_backend.rs`.

### `aida-core/src/db/cache.rs` — verify, no code change expected

- Cache writes `format!("{:?}", req.status)` (Debug form, i.e. `"Done"`). Filter normalization already lowercases. **Verify** the row-builder near `INSERT INTO requirements_cache` uses Debug form, not Display ("In Progress" vs "InProgress").

### `aida-cli/src/main.rs` tests

- `is_terminal_status_buckets`: assert `!is_terminal_status(&RequirementStatus::Done)`.

### `aida-core/templates/skills/aida-*.md` + `CLAUDE.md` — docs

- `aida-pickup.md`, `aida-implement.md`, `aida-commit.md`, `aida-pr.md`, `aida-status.md`: replace `InProgress → Completed` with `InProgress → Done → Completed`. Mechanical; grep `grep -l "Completed" aida-core/templates/skills/*.md`.
- `CLAUDE.md` "Status values" section: add `done` between `in-progress` and `completed`. Add: "Done = finished on branch; Completed = merged to main. `aida pull` auto-bumps Done→Completed on default branch."
- META prompts (`META-002`, `META-006`, etc.): edit any that name the lifecycle explicitly. `aida list --type meta` then `aida show META-NNN`.

### React UI (generated + a few constants)

- Regenerate `shared/types.ts` via `aida-generate-types`. Confirms `RequirementStatus` adds `"Done"`.
- `aida-web-react/src/lib/constants.ts`:
  - `STATUS_ORDER`: insert `'Done'` between `'InProgress'` and `'Completed'`.
  - `STATUS_CONFIG`: add `Done: { color: 'text-lime-400', bg: 'bg-lime-500/10', dot: 'bg-lime-400', label: 'Done' }`.
- `aida-web-react/src/components/kanban/KanbanBoard.tsx` (`emptyColumns`, `collapsedStatuses` defaults): add `Done: []` / `Done: false`.
- `aida-web-react/src/components/queue/QueuePage.tsx`: `showCompleted` filter currently hides `status !== 'Completed'`. Update to also keep Done items hidden by default. Rename label to "Show done/completed".

## Critical Files

- `aida-core/src/models.rs` (enum, set_status_from_str, ImplementationInfo)
- `aida-cli/src/main.rs` (validate_status_input, is_terminal_status_str, is_terminal_status, update_manifest_for_status, handle_pull_command, DbCommand::Sync, queue done handler, queue color renderer, extract_spec_ids_from_commit reuse)
- `aida-cli/src/history.rs` (is_terminal_status_str twin, colorize_status)
- `aida-cli/src/session_manifest.rs` (`fn classify_item` — verify Done falls through, no edit needed)
- `aida-core/src/import.rs` (parse_requirement_status)
- `aida-core/src/db/sqlite_backend.rs`, `aida-core/src/db/postgres_backend.rs` (status_to_str / str_to_status pairs)
- `aida-core/src/db/cache.rs` (verify Debug-form casing)
- `aida-web-react/src/lib/constants.ts`, `aida-web-react/src/components/kanban/KanbanBoard.tsx`, `aida-web-react/src/components/queue/QueuePage.tsx`
- `aida-core/templates/skills/*.md` (mechanical doc updates)
- `shared/types.ts` (regenerated via aida-generate-types)
- `CLAUDE.md` (root)

## Reusable helpers (do not reimplement)

- `extract_spec_ids_from_commit` (`aida-cli/src/main.rs`) — parses `(REQ-ID)` from commit subjects, already handles multi-spec parens + `(#N)` PR suffixes.
- `git_ops::head_sha`, `git_ops::current_branch`, `git_ops::has_remote` (`aida-core/src/git_ops.rs`).
- `record_role_activity` — already used by `QueueCommand::Done` and BUG-65 fix.
- `Storage::update_atomically` — works across SQLite + git-canonical.
- `Requirement::set_status_from_str` (after we add the `"done"` arm) — use this rather than assigning the enum directly so any custom_status cleanup runs.
- `print_pull_summary` already extracts status transitions via `extract_status_changes_from_commits` and prints them grouped — the auto-bump's own flips will surface there too once committed to the orphan store, so the user-facing "Done → Completed" line comes for free. Don't double-print.

## Risks + gotchas

1. **Risk**: Serde forward compat (downgrade) — a YAML file with `status: Done` written by the new binary fails to deserialize on an older binary (unknown enum variant). **Mitigation**: Acceptable for a workspace tool; no `#[serde(other)]` fallback (would silently mis-route Done into a default and break the auto-bump invariant).
2. **Risk**: Pre-existing Completed reqs (spec acceptance #6). **Mitigation**: Treat as already-shipped. No migration. The auto-bump scan only matches `status == Done`, so existing Completed is unaffected.
3. **Risk**: In-flight `Completed` on un-merged branches — anyone with a Completed req sitting on a branch loses the Done/Completed distinction for that one. **Mitigation**: Cleanup is manual (`aida edit ID --status done`) and optional. Document in commit message; don't auto-rewrite.
4. **Risk**: Default branch detection — `git symbolic-ref refs/remotes/origin/HEAD` only works when the remote was cloned. **Mitigation**: Fall back to `current_branch` ∈ {`main`, `master`}. If neither, skip auto-bump silently.
5. **Risk**: Squash + rebase + cherry-pick. **Mitigation**: The scan keys off `(REQ-ID)` in the commit subject — survives squash and cherry-pick. Force-pushes that rewrite history can drop a previously-scanned commit, but the bump already happened (still correct in the merged-state-of-the-world sense).
6. **Risk**: Reverted commits — the helper does not look at `Revert "..."` subjects. **Mitigation**: Out of scope per spec. File as followup.
7. **Risk**: First pull on a new clone — `pre_sha` is `None`; helper scans last 50 commits. **Mitigation**: The scan only flips reqs currently in `Done`. On a fresh clone, the orphan store likely has these reqs already as Completed (bumped on the original machine); the `status == Done` guard prevents accidents.
8. **Risk**: Cache "InProgress" vs "In Progress" casing mismatch. **Mitigation**: Cache stores Debug form (verified). Don't accidentally normalize to Display form anywhere; that breaks every existing query.

## Tests (named, not "add tests")

- `auto_bump_done_to_completed_picks_up_subject_refs` — happy path.
- `auto_bump_skips_when_not_on_default_branch` — negative case.
- `queue_done_flips_to_done_not_completed` — invariant on the queue done handler.
- `is_terminal_status_buckets` (existing, extended) — assert `!is_terminal(Done)`.
- `colorize_status_done_is_bold_green` — visual differentiation from plain Completed.
- (3 additional helper tests as needed for default-branch detection edge cases.)

## Verification

```bash
# Build + lint
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p aida-core -p aida-cli

# End-to-end lifecycle in a temp project
TMP=$(mktemp -d); cd "$TMP" && git init && aida init
aida add --title "auto-bump smoke" --type story --status approved
SPEC=$(aida list --status approved --json | jq -r '.[0].spec_id')
aida edit "$SPEC" --status in-progress
aida queue add "$SPEC"
aida queue done "$SPEC" --yes
aida show "$SPEC" | grep -i status      # expect: Done
echo "fix: bump test ($SPEC)" >file.txt
git add file.txt
git commit -m "fix: bump test ($SPEC)"
aida pull --quiet                       # on default branch — should auto-bump
aida show "$SPEC" | grep -i status      # expect: Completed
aida show "$SPEC" | grep -i completion_sha   # expect: matching SHA

# Negative test — feature branch should NOT auto-bump
git checkout -b feature
aida edit "$SPEC" --status done
git commit --allow-empty -m "feat: shouldn't bump ($SPEC)"
aida pull --quiet
aida show "$SPEC" | grep -i status      # expect: Done (still)

# Manual list + history check
aida list --status done                 # shows the Done item
aida history --all                      # Done renders distinct from Completed
```

## Followups (file as TASKs at completion time)

- Reverted-commit handling: `Completed → Done` (or `Completed → Approved`) when a referencing commit is reverted on main.
- Statusline `@SPEC` color — should Done look different from Completed in the prompt? Coordinate with STORY-78/57 owners.
- `aida db reconcile-status` manual subcommand for cold-start replay (later filed as TASK-226 and shipped).

## Related

- **STORY-86** — New status 'Done': distinguish 'work finished on branch' from 'merged to main' (Completed)
- Builds on: **EPIC-21** — Code↔store commit pairing (provides Aida-Store trailer infrastructure)
- Composes with: **STORY-81** (implementation_info auto-populate), **TASK-43** (aida pull as hook point), **BUG-65** (activity log on edit/done)
- See also: `docs/positioning/vs-karpathy-md.md` (status-machine identity is one of AIDA's defensible niches)

---

*Plan generated 2026-05-13 via /ultraplan; retrofitted 2026-05-14 to the
structured plan template (TASK-92). Original prose preserved, sections
re-shaped to match `docs/plans/_TEMPLATE.md`.*
