# STORY-86 — Add `Done` status between `InProgress` and `Completed`

## Status

Done — implemented across 4 commits on `epic-21-2` (2026-05-13):
- `de3cc1d5` enum variant + serde wiring
- `a6f25289` queue done flip + history/manifest
- `3b0ff9df` auto-bump helper + pull/sync wiring + 8 new tests
- `224950b8` React UI + skill templates + CLAUDE.md + META prompts

Verification: full workspace test suite (231 + 278) passes; release
binary built and `--status done` filter works against the live store.
End-to-end: STORY-86 itself flipped to `Done` via `aida queue done` —
will auto-bump to `Completed` once `aida pull` runs on main after PR merge.

## Related Requirements

- **STORY-86** — New status 'Done': distinguish 'work finished on branch' from 'merged to main' (Completed)
- **EPIC-21** — Code↔store commit pairing (parent — provides Aida-Store trailer infrastructure)
- **STORY-81** — implementation_info auto-populate on completion (composes — natural to populate completed_at + completion_sha at auto-bump moment)
- **TASK-43** — aida pull (composes — natural hook point for auto-bump scan)
- **BUG-65** — activity log on edit/done (composes — auto-bump should record_role_activity too)

## Context

`aida history`, `aida show`, and the kanban board all surface `Completed` for two semantically different states:

1. **Done on a branch** — an agent ran `aida queue done`, but the PR hasn't merged yet.
2. **Merged to main** — actually shipped, visible to everyone.

Today the only difference between (1) and (2) is hidden in git. STORY-86 splits them: `aida queue done` flips to a new `Done` status, and the existing post-pull pipeline auto-bumps `Done → Completed` when a referencing commit lands on the default branch. `Completed` keeps its existing semantics ("shipped on main"), so existing data needs no migration.

## Approach (one diagram, one paragraph)

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

Implementation is contained: one new enum variant, one new helper `auto_bump_done_to_completed` reused by `aida pull` and `aida db sync --pull`, and a small set of display/serde call-site updates.

## File-by-file changes

Order matters — do edits top-to-bottom so each commit builds.

### 1. `aida-core/src/models.rs` — enum + setters

- **Add variant** between `InProgress` and `Completed`:
  ```rust
  pub enum RequirementStatus {
      Draft, Approved, Planned, InProgress, Done, Completed, Rejected,
  }
  ```
- **`Display`** (line ~21): add `RequirementStatus::Done => write!(f, "Done")`.
- **`set_status_from_str`** (line ~3174): add `"done" => self.status = RequirementStatus::Done; self.custom_status = None;`. Note: `"done"` previously fell through to `custom_status` — verify no test relies on that.
- **`ImplementationInfo`** (line ~2371): add two optional fields, both `#[serde(default, skip_serializing_if = "Option::is_none")]`:
  - `completed_at: Option<DateTime<Utc>>` — set when auto-bump fires.
  - `completion_sha: Option<String>` — git SHA on the default branch that triggered the bump.
  Update `Default` + `ImplementationInfo::new` accordingly.

### 2. `aida-cli/src/main.rs` — validation, terminal checks, queue done, colors, auto-bump

- **`validate_status_input`** (line ~3175): add `"done" => Ok("Done")`. Remove the `"done" => Ok("Completed")` alias — `done` is now its own state. Update the error message's value list.
- **`is_terminal_status_str`** (line ~3153): **do not** add `Done` — `Done` is non-terminal (it can advance to `Completed`). Today the function returns true for `"done"` (treats agent-friendly synonym as Completed). Remove the `"done"` branch. The history.rs twin (line ~637) needs the same removal.
- **`is_terminal_status`** (line ~4951): unchanged — keep `Completed | Rejected` only.
- **`QueueCommand::Done` handler** (line ~28412): change `r.set_status_from_str("Completed")` to `r.set_status_from_str("Done")`. Update the prompt text ("Mark X as **done** and remove from queue?") and the success message ("X marked **done** and removed from queue."). Keep the `implementation_info.implemented = true` stamping (this still means "work-on-branch is done"); leave `completed_at` / `completion_sha` unset here (auto-bump sets them on merge).
- **`update_manifest_for_status`** (line ~14083): add the `"done"` branch so it calls `session_manifest::mark_completed` (the session manifest only tracks "is this item visually checked off", and Done qualifies). Keep `"completed"` and `"rejected"` mapping to `mark_completed` as well.
- **Queue list color renderer** (lines 27377 and 27500): add `"Done" => status.bold().green()` (or `.bright_green()`) for visual distinction from `Completed` (plain green). Same two sites.
- **`update_manifest_for_status` callsite** (line ~28502): change to `update_manifest_for_status(spec_id, "Done")`.
- **`handle_pull_command`** (line ~19713):
  - Snapshot `let pre_code_sha = git_ops::head_sha(&project_root).ok();` **before** the `git pull --ff-only` call.
  - After a successful code pull, call new helper `auto_bump_done_to_completed(&project_root, store_path, pre_code_sha.as_deref(), storage)`.
  - Render summary inline ("auto-bumped N Done → Completed" + listing first 5 spec_ids).
  - Skip when not on the default branch (helper handles this — see below).
- **`Command::Db(DbCommand::Sync { pull, .. })`** (line ~2846): same hook after `pull_rebase` succeeds. But this path is **orphan-store pull**, not code pull. The auto-bump scan needs to look at the **code repo's** default branch, not the orphan branch. Take the snapshot/scan of the **code repo** here too — it's typical the user has just pulled both. Use the same helper.
- **New helper `auto_bump_done_to_completed`** (place near `print_pull_summary` ~line 19886):
  ```rust
  fn auto_bump_done_to_completed(
      project_root: &Path,
      store_path: &Path,
      pre_sha: Option<&str>,   // None ⇒ scan HEAD only (first pull)
      storage: &Storage,
  ) -> Result<Vec<(String, String)>> { ... }
  ```
  Steps:
  1. Detect default branch: try `git symbolic-ref --short refs/remotes/origin/HEAD` → strip `origin/` prefix; fall back to `main`/`master` by `current_branch`. If current branch ≠ default branch, return `Ok(vec![])` (silently skip).
  2. Build the commit range: `pre_sha..HEAD` if known, else `HEAD~50..HEAD` (cap on first-pull / shallow clones).
  3. `git -C project_root log --pretty=format:%H%x09%s%x09%(trailers:key=Aida-Store,valueonly,separator=%x00) range` — one row per commit. Subject + optional `Aida-Store: <store-sha>` trailer.
  4. For each row: collect spec IDs via existing `extract_spec_ids_from_commit(subject)` (line 24857). Track `spec_id → (commit_sha, store_sha)` map; first commit wins.
  5. Load store via `storage.load()?`; for each candidate spec, only flip if current status is exactly `RequirementStatus::Done`. Idempotent: silently skip any spec already at `Completed`/`Rejected` or anything else.
  6. `storage.update_atomically(|s| { ... })`: set status = Completed, set `implementation_info.completed_at = Some(Utc::now())`, set `implementation_info.completion_sha = Some(commit_sha)`. Don't overwrite `implemented_at` / `implemented_by` (those were stamped by `queue done`).
  7. After the write, for each flipped spec call `record_role_activity(spec_id, "auto-completed")` (mirrors the BUG-65 hookup in `QueueCommand::Done`).
  8. Return `Vec<(spec_id, commit_sha)>` so caller can print a summary.
- **`AIDA_AUTO_BUMP` env opt-out**, mirroring `auto_merge_gate_enabled` (line 19869). Default on; respect `AIDA_AUTO_BUMP=false|0|no|off`.

### 3. `aida-cli/src/history.rs` — colorize + terminal check

- **`colorize_status`** (line 686): add `"Done" => status.bold().green().to_string()`. Keep `"Completed" => status.green()` plain.
- **`is_terminal_status_str`** (line 637): remove the `t.eq_ignore_ascii_case("done")` clause — Done is no longer terminal.
- The `--all` flag's user-facing text ("pass --all to see Completed/Rejected items") stays correct.

### 4. `aida-core/src/import.rs` — explicit parser table

- `parse_requirement_status` (line 380): add `"Done" => Some(RequirementStatus::Done)` and the missing `"Planned"` and `"In Progress"`/`"InProgress"` rows while you're there (current table is incomplete — it works because YAML/serde is the primary path).

### 5. `aida-core/src/db/sqlite_backend.rs` + `aida-core/src/db/postgres_backend.rs`

- `status_to_str` (line 288): add `RequirementStatus::Done => "Done"`.
- `str_to_status` (line 300): add `"Done" => RequirementStatus::Done`.
- Apply the parallel two-line change in `postgres_backend.rs` (same shape).

### 6. `aida-core/src/db/cache.rs` — no code change

Storage column is `TEXT`; the cache writes `format!("{:?}", req.status)` (i.e. `"Done"`). Filter normalization already lowercases / strips punctuation, so `aida list --status done` matches a stored `"Done"` row without any new code. **Verify** the column population path: locate the row-builder (look for `req_type` / `status` write near `INSERT INTO requirements_cache`) and confirm it uses Debug form, not Display ("In Progress" vs "InProgress"). Adjust if needed.

### 7. `aida-cli/src/main.rs` tests

- `is_terminal_status_buckets` (line 15491): assert `!is_terminal_status(&RequirementStatus::Done)`.
- New test: `auto_bump_done_to_completed_picks_up_subject_refs` — set up a temp project (`git init`, `aida init`), add a req in Draft → Done, commit `feat: x (STORY-X)` on main, call helper, assert status == Completed and `completion_sha` matches.
- New test: `auto_bump_skips_when_not_on_default_branch` — same setup, check out a feature branch before invoking helper, assert no flip.
- New test: `queue_done_flips_to_done_not_completed` — invoke `QueueCommand::Done`, assert stored status is `Done`.

### 8. `aida-core/templates/skills/aida-*.md` + `CLAUDE.md` — docs

- `aida-pickup.md`, `aida-implement.md`, `aida-commit.md`, `aida-pr.md`, `aida-status.md`: anywhere they mention the lifecycle, replace `InProgress → Completed` with `InProgress → Done → Completed`. The list is mechanical; grep `grep -l "Completed" aida-core/templates/skills/*.md`.
- `CLAUDE.md` (this repo, top of file): under "Status values" section if present, add `done` between `in-progress` and `completed`. Add a one-line callout: "Done = finished on branch; Completed = merged to main. `aida pull` auto-bumps Done→Completed on default branch."
- META prompts (`META-002` Evaluate, `META-006` Generate Children, etc.) — only if they enumerate statuses verbatim. Run `aida list --type meta` then `aida show META-NNN` for each; edit any that name the lifecycle explicitly.

### 9. React UI (generated + a few constants)

- Regenerate `shared/types.ts` via the existing `aida-generate-types` workflow (`cargo run -p aida-generate-types` or whatever target is wired). Confirms `RequirementStatus` adds `"Done"`.
- `aida-web-react/src/lib/constants.ts`:
  - `STATUS_ORDER`: insert `'Done'` between `'InProgress'` and `'Completed'`.
  - `STATUS_CONFIG`: add `Done: { color: 'text-lime-400', bg: 'bg-lime-500/10', dot: 'bg-lime-400', label: 'Done' }` (or whatever palette token reads as "almost complete"). Place between `InProgress` and `Completed` for grep-friendliness.
- `aida-web-react/src/components/kanban/KanbanBoard.tsx` (`emptyColumns`, `collapsedStatuses` defaults at line 35 and 72): add `Done: []` / `Done: false`.
- `aida-web-react/src/components/queue/QueuePage.tsx` (line 64): the `showCompleted` filter currently hides `status !== 'Completed'`. Update to hide `status !== 'Completed' && status !== 'Done'` so Done items behave like the previous Completed default. The checkbox label can stay "Show completed" or rename to "Show done/completed" — small judgement call; default to expanding it.

## Where the auto-bump scan lives — decision

**Both.** Same helper, two callsites:

- `handle_pull_command` (`aida pull`) — fires after the code-pull leg succeeds.
- `Command::Db(DbCommand::Sync { pull: true, .. })` — fires after `pull_rebase` of the orphan store succeeds. This is the older path; users who scripted around `aida db sync --pull` keep working.

The helper itself looks at the **code repo's** default branch (where merges happen), not the orphan branch. That's the same in both callsites — neither needs a code-vs-store branch.

No separate `aida db reconcile-status` subcommand for v1. If we later want manual replay (e.g., for backfill on a fresh clone), wrap the helper as a hidden subcommand then; not required to ship.

## Risks + gotchas

1. **Serde forward compat (downgrade)**: A YAML file with `status: Done` written by the new binary will fail to deserialize on an older binary (unknown enum variant). Acceptable for a workspace tool — we're not shipping a library API. Don't add a `#[serde(other)]` fallback; that would silently mis-route Done into a default and break the auto-bump invariant.
2. **Pre-existing Completed reqs (spec acceptance #6)**: Treat as already-shipped. No migration. The auto-bump scan only matches `status == Done`, so existing Completed is unaffected.
3. **In-flight `Completed` on un-merged branches**: After upgrade, anyone who already had a Completed req sitting on a branch loses the Done/Completed distinction for that one. Cleanup is manual (`aida edit ID --status done`) and optional. Document in commit message; don't auto-rewrite.
4. **Default branch detection**: `git symbolic-ref refs/remotes/origin/HEAD` only works when the remote was cloned (it's set at clone time). For `aida init` projects without a remote, fall back to `current_branch` ∈ {`main`, `master`}. If neither, skip auto-bump silently — no remote means no merges to react to.
5. **Squash + rebase + cherry-pick**: The scan keys off `(REQ-ID)` in the commit subject — survives squash (which preserves subject) and cherry-pick. Force-pushes that rewrite history on `main` can drop a previously-scanned commit, but since the bump already happened the spec stays Completed (which is still correct in the merged-state-of-the-world sense).
6. **Reverted commits**: Out of scope per spec. The helper does not look at `Revert "..."` subjects. File a followup TASK at completion time.
7. **First pull on a new clone**: `pre_sha` is `None`; helper scans last 50 commits. Risk of bumping legitimately-pending Done reqs that were intentionally left un-completed. Mitigation: the scan only flips reqs currently in `Done`. On a fresh clone, the orphan store likely has these reqs already as Completed (they were bumped on the original machine), so the `status == Done` guard prevents accidents.
8. **Cache "InProgress" vs "In Progress"**: The cache stores Debug form (verified — `cache.rs` lines 244-248 comment). Don't accidentally normalize to Display form (`"In Progress"`) anywhere; that breaks every existing query.

## Complexity estimate

- **Prod LOC**: ~250 (helper ~80, models.rs ~10, queue done ~5, history.rs ~5, sqlite/postgres backends ~6, pull/sync wiring ~30, React ~30, docs/templates ~80).
- **Test LOC**: ~150 (3 helper tests, 1 queue test, 1 history color test).
- **Commits**: 4 — (1) enum + setters + display/parsers, (2) queue done flip + history/manifest, (3) auto-bump helper + pull/sync wiring + tests, (4) React + docs + META prompts.
- **Risk surface**: medium-low. Largest unknown is the cache row-builder casing — confirm once.

## Critical files (paths)

- `aida-core/src/models.rs:12` (enum), `:3174` (set_status_from_str), `:2371` (ImplementationInfo)
- `aida-cli/src/main.rs:3153` (terminal-str), `:3175` (validate), `:4951` (terminal-enum), `:14083` (manifest mapping), `:19713` (handle_pull_command), `:2846` (db sync pull), `:27377` and `:27500` (queue color), `:28412` (queue done), `:24857` (existing extract_spec_ids_from_commit — reuse)
- `aida-cli/src/history.rs:637` (terminal-str twin), `:686` (colorize_status)
- `aida-cli/src/session_manifest.rs:228` (classify_item — verify Done falls through, no edit needed)
- `aida-core/src/import.rs:380` (parser table)
- `aida-core/src/db/sqlite_backend.rs:288` + `aida-core/src/db/postgres_backend.rs:~252` (status_to_str/str_to_status pairs)
- `aida-core/src/db/cache.rs:244` (verify the Debug-form casing comment matches the row writer)
- `aida-web-react/src/lib/constants.ts:3-14`, `aida-web-react/src/components/kanban/KanbanBoard.tsx:35,72`, `aida-web-react/src/components/queue/QueuePage.tsx:64`
- `aida-core/templates/skills/*.md` (mechanical doc updates)
- `shared/types.ts` (generated — regenerate via aida-generate-types)
- `CLAUDE.md` (root — doc the new lifecycle)

## Reusable helpers (do not reimplement)

- `extract_spec_ids_from_commit` (`aida-cli/src/main.rs:24857`) — parses `(REQ-ID)` from commit subjects, already handles multi-spec parens + `(#N)` PR suffixes.
- `git_ops::head_sha`, `git_ops::current_branch`, `git_ops::has_remote` (`aida-core/src/git_ops.rs`).
- `record_role_activity` (already used by `QueueCommand::Done` and BUG-65 fix).
- `Storage::update_atomically` (works across SQLite + git-canonical).
- `Requirement::set_status_from_str` (after we add the `"done"` arm) — use this rather than assigning the enum directly so any custom_status cleanup runs.
- `print_pull_summary` already extracts status transitions via `extract_status_changes_from_commits` (line 19987) and prints them grouped — the auto-bump's own flips will surface there too once they're committed to the orphan store, so we get the user-facing "Done → Completed" line for free. Don't double-print; the helper's summary should focus on "auto-bumped" specifically.

## Verification (end-to-end)

```bash
# Build + lint
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p aida-core -p aida-cli

# Unit-test the lifecycle in a temp project
TMP=$(mktemp -d); cd "$TMP" && git init && aida init
aida add --title "auto-bump smoke" --type story --status approved
SPEC=$(aida list --status approved --json | jq -r '.[0].spec_id')  # adjust to actual flag
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

# React UI smoke — only if React was touched
(cd aida-web-react && npm run dev) &
# Open kanban, drag-drop into the new Done column, refresh, confirm persistence
```

## Followups (file as tasks, do not implement)

- Reverted-commit handling: `Completed → Done` (or `Completed → Approved`) when a referencing commit is reverted on main.
- Statusline `@SPEC` color: should Done look different from Completed in the prompt? Out of scope; coordinate with STORY-78/57 owners.
- `aida db reconcile-status` manual subcommand for cold-start replay.

---

*Plan generated 2026-05-13 via /ultraplan. Saved to docs/plans/ per AIDA convention. Ready for implementer pickup.*
