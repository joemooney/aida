# Plan: STORY-385 — `aida status --cleanup`

Date: 2026-05-24
Specs: STORY-385
Status: In Progress
Complexity: ~800 prod LOC, ~250 test LOC, 1 commit, risk low

## Approach

Add a `--cleanup` flag to `aida status` that surfaces eight categories of
attention-actionable state currently spread across `git worktree list`,
`aida session leases --all`, `gh pr list`, `git status`, and recent
`git log`. The eight categories are detected independently — each
graceful-degrades on missing inputs — and rendered in stakes order
(loss-risky first → pure-cleanup last). Default `aida status` (no flag)
gets a one-line summary at the bottom when the cleanup section is
non-empty; otherwise the status surface is unchanged.

The detection layer lives in `main.rs` next to the existing lease /
git / gh helpers it reuses (`list_leases`, `classify_lease_state`,
`worktree_dirty_entries`, `extract_spec_ids_from_commit`,
`resolve_gh_binary`, `detect_default_branch_ref`). The data shape +
rendering live in a new `status_cleanup` module so the renderer is
unit-testable without filesystem access.

```
         ┌──────────────┐
   gh ──►│              │
git list │  collect_    │   ┌──────────────┐
git log ─┼─►cleanup_    ├──►│ CleanupReport│
leases ──┘  report      │   │  + render()  │──► stdout (text)
                        │   │  + to_json() │──► stdout (json)
                        └───┴──────────────┘──► summary_line() ──► appended
                                                                   to default
                                                                   `aida status`
```

## Decisions

- **Original Acceptance only this slice**: the 8 categories in the spec's
  Acceptance section + `--cleanup` / `--cleanup --json` / `--cleanup
  --verbose`. The four comment refinements (findings tally, full worktrees
  section, working-tree state with persistence files, advisor-activity log)
  are deferred to follow-up TASKs per memory
  `feedback_refinements_must_be_acceptance_criteria` — comments are not
  binding unless they edit the acceptance criteria. **Rationale**:
  smallest valuable slice; the comment additions are each ~STORY-sized in
  their own right and would balloon the PR past reviewable size.
- **Detection in main.rs, rendering + shape in `status_cleanup.rs`**:
  detectors call module-private helpers like `SessionLease`,
  `LeaseState`, `worktree_dirty_entries` that already live in main.rs.
  Moving them out for testability isn't worth the churn. The
  `status_cleanup` module owns the report struct + render/json, which
  *is* worth isolating — its 6 unit tests cover every category renderer
  + the JSON contract + the cap + the Healthy footer.
  **Rationale**: minimal disruption to the existing 67k-line main.rs;
  pure logic stays testable.
- **Stakes ordering**: 1) Uncommitted WIP (loss-risky); 2) Sticky
  In-Progress without lease; 3) Branches ahead of main with no PR;
  4) Missed auto-bump; 5) Open PRs awaiting merge; 6) Dormant leases;
  7) Stale reviewer leases on merged PRs; 8) Orphan project dirs.
  **Rationale**: matches the spec's "loss-risky first" instruction;
  pure-cleanup categories sink to the bottom so the eye lands on the
  expensive-to-lose items first.
- **Cap at 3 per category, `--verbose` lifts**: the spec's "first N
  items (max 3)" was set in stone in the Acceptance section. The
  overflow line names the verbose flag explicitly so it's discoverable.
- **Dedupe stale-reviewer-on-merged out of Dormant**: a reviewer lease
  for a merged PR is technically also dormant; surfacing twice is noise.
  Stale-reviewer is the more actionable signal, so suppress it from
  Dormant. **Rationale**: avoid double-counting in the Total line.
- **`branches_ahead_no_pr` filters**: orphan store branch (`aida-store`)
  excluded; reviewer worktree branches (`pr-N` / `mr-N`) excluded
  (they surface as stale-reviewer); non-spec-pattern branches excluded
  (long-lived feature branches and ad-hoc `claude/plan-...` agent-tool
  branches aren't what this category is for); branches with no commit in
  the last 30 days excluded; branches that have EVER had a PR (open /
  closed / merged) excluded — squash-merged branches' local tips look
  "ahead" of main forever otherwise. **Rationale**: empirical noise
  reduction — without these filters the test repo produced 230 items
  (squash-merge artifacts); with them, 3 (the actual signal).
- **EPIC / Vision / Folder / Meta / Sprint excluded from sticky
  In-Progress**: parent specs flip to In Progress when their children
  are working; they have no branch and aren't "stuck". **Rationale**:
  matches what the operator considers a recovery-actionable In-Progress.
- **`--cleanup` collection runs on default `aida status` too**: the
  spec's summary-line requirement implies running the scan. Cost is
  comparable to existing `gh` calls already in default status; both
  detectors share the same one-shot `gh pr list` invocations.

## Files (in build-order)

### `aida-cli/src/cli.rs` — Command::Status flags

- `Command::Status`: add `cleanup: bool` (long-only, conflicts with
  `--queue` / `--ci` / `--short`) and `verbose: bool` (long-only,
  requires `--cleanup`).

### `aida-cli/src/status_cleanup.rs` — new module

- `CleanupReport`: struct with one `Vec<*Item>` per category.
- `*Item`: 8 small structs (`UncommittedWipItem`, `StickyInProgressItem`,
  `BranchAheadItem`, `MissedAutoBumpItem`, `OpenPrItem`,
  `DormantLeaseItem`, `StaleReviewerLeaseItem`, `OrphanProjectDirItem`).
- `CleanupReport::total`, `is_empty`, `summary_line`, `to_json`,
  `render(verbose, w)`.
- 8 per-category render helpers — each prints "⚠ <heading> (N):", up to
  `cap` lines, an overflow indicator, then the recovery verb.
- `tests`: empty-report all-clear, summary-line gating, each-category
  prints recovery verb, cap-without-verbose vs verbose, Healthy footer
  enumerates empty categories, JSON shape round-trip.

### `aida-cli/src/main.rs` — wiring + detectors

- `mod status_cleanup;` declaration.
- `Command::Status { … cleanup, verbose, … }` destructure (twice — both
  the legacy and distributed match arms).
- `handle_status_command_distributed` — accept `cleanup` + `verbose`;
  short-circuit to `collect_cleanup_report` + render/json when
  `cleanup`.
- Default `aida status` tail: call `collect_cleanup_report` and append
  the `summary_line()` when non-empty.
- New helpers (kept in main.rs because they reuse SessionLease /
  LeaseState / module-private git helpers):
  - `list_worktrees(project_root) -> Vec<WorktreeRecord>`
  - `collect_open_prs(project_root) -> OpenPrSnapshot`
  - `summarize_status_check_rollup(checks) -> String`
  - `collect_all_pr_head_branches(project_root) -> HashSet<String>`
  - `scan_default_branch_for_spec_landings(project_root, limit)`
  - `branch_ahead_of(project_root, branch, target)`
  - `list_local_branches(project_root)`
  - `gh_pr_is_merged(project_root, n)`
  - `collect_orphan_project_dirs() -> Vec<OrphanProjectDirItem>`
  - `parse_pr_scope(scope) -> Option<u64>`
  - `is_work_spec_branch_name(branch) -> bool` (+ unit tests)
- `collect_cleanup_report(project_root, store, _backend)` — runs the 8
  detectors, dedupes stale-reviewer out of Dormant.

## Critical Files

- `aida-cli/src/cli.rs`
- `aida-cli/src/main.rs`
- `aida-cli/src/status_cleanup.rs` (new)

## Reusable helpers (do not reimplement)

- `list_leases(project_root)` (main.rs) — read every `*.toml` under
  `.aida/sessions/leases/`.
- `classify_lease_state(worktree_exists, has_live_claude, age_hours)`
  (main.rs) — TASK-55 lease state machine; `LeaseState::Live` /
  `Dormant` / `Stale`.
- `worktree_dirty_entries(path)` (main.rs) — BUG-67 `git status
  --porcelain` line count.
- `process_probe::probe_live_claude_sessions()` — live process snapshot
  for lease classification.
- `detect_default_branch_ref(project_root)` (main.rs) — BUG-76 default
  branch ref resolution.
- `extract_spec_ids_from_commit(subject)` (main.rs) — STORY-86 commit
  subject parsing; reused by the missed-auto-bump scanner.
- `resolve_gh_binary()` (main.rs) — graceful `gh` detection.
- `aida_core::models::RequirementsStore::get_requirement_by_spec_id` —
  spec lookup by ID.
- `RequirementStatus` / `RequirementType` enums (aida_core).

## Risks + gotchas

1. **Risk**: `gh pr list --state all --limit 500` may not cover every
   historical PR head — repos with >500 closed PRs will still have
   false positives in branches-ahead-no-PR. **Mitigation**: combined
   with the work-spec-branch-name filter and 30-day recency filter, the
   residual noise in this repo dropped from 227 to 3 items, all real
   signal.
2. **Risk**: The summary-line scan on default `aida status` adds gh
   round-trip latency to every invocation. **Mitigation**: detectors
   graceful-degrade on missing `gh`, so users without `gh` installed
   pay nothing extra; the per-detector cost is comparable to the
   existing PR/CI block already in default status.
3. **Risk**: A spec marked In Progress that doesn't follow the
   `<type>-<n>` branch convention will report `branch (no branch)`
   even when the user named the branch differently. **Mitigation**:
   accepted — the convention is well-established and the spec's
   recovery verb (`aida queue recover <spec>`) doesn't care about the
   branch name.
4. **Risk**: `colored::control::set_override` is process-global and
   non-thread-safe under parallel tests. **Mitigation**: the tests use
   `colored::control::set_override(false)` + `unset_override()` and
   compare against ANSI-stripped output, not raw bytes — even if a
   parallel test re-enables colour, the strip handles it.

## Tests (named, not "add tests")

- `status_cleanup::empty_report_renders_all_clear` — zero items prints
  the all-clear, no Healthy block, no Total line.
- `status_cleanup::summary_line_present_when_non_empty` — gating.
- `status_cleanup::each_category_prints_recovery_verb` — every of the
  8 categories shows its recovery verb.
- `status_cleanup::cap_applied_without_verbose` — 5 items → cap=3 →
  "(2 more — pass --verbose to show all)"; verbose lifts.
- `status_cleanup::healthy_footer_lists_empty_categories` — 1 populated
  category → 7 healthy lines.
- `status_cleanup::json_shape_round_trips_total_and_arrays` —
  `categories.<key>` is always an array (never missing).
- `is_work_spec_branch_name_tests::matches_canonical_work_branches` —
  accepts `task-281`, `epic-20-batch7`, etc.
- `is_work_spec_branch_name_tests::rejects_pr_and_mr_branches` —
  excludes reviewer-worktree branches.
- `is_work_spec_branch_name_tests::rejects_non_spec_branches` — rejects
  `main`, `feature/login`, `claude/plan-X`, `aida-store`.

## Verification

```bash
cargo build -p aida-cli --release

# Default status: silent unless cleanup-actionable
aida status --no-dev-context | tail -3
# Expect: "⚠ N items need cleanup attention — `aida status --cleanup` for details"
#         when N > 0, otherwise no extra line.

# --cleanup text view: stakes-ordered, capped at 3 with overflow line
aida status --cleanup | head -50
# Expect: "─── Needs attention ───", category headings with "⚠", capped
#         lists, recovery verbs, "─── Healthy ───" footer, total line.

aida status --cleanup --verbose | grep "more — pass --verbose"
# Expect: no matches (verbose disables the cap).

# --cleanup --json: machine-readable
aida status --cleanup --json | jq '.total, (.categories | keys)'
# Expect: total integer, 8-key category map.
```

## Followups

- Findings tally on default `aida status` + Findings category in the
  cleanup section (per the 2026-05-21 comment refinement; deferred).
- Full Worktrees section in `aida status` — every worktree, obsolescence
  verdict, lease state, PR linkage (per the 2026-05-22 comment
  refinement; deferred — significant detection surface of its own).
- Working-tree state (modified / untracked-recent / untracked-stale)
  with `.aida/last-status.toml` + `.aida/untracked-history.toml`
  persistence (per the 2026-05-22 comment refinement; deferred —
  introduces new persistence files + heuristic rules).
- Advisor-activity log surface (per the 2026-05-22 multi-agent
  collision comment; deferred).
- `aida status --cleanup --resolve` to integrate the move-pull-diff
  cleanup dance for untracked files blocking `aida pull` (per the
  2026-05-22 priority-bump comment).
- Distinguish "latest CI run on origin HEAD" from "historical runs on
  prior commits" in the Open PRs category (per the 2026-05-22
  `gh pr checks --watch` stale-display observation).
- `aida status --cleanup` performance — the gh + git invocations could
  be parallelised if the cleanup-on-default-status cost becomes a
  concern.

## Related

- Builds on: STORY-385 spec, TASK-220 (unified `aida status` view).
- Composes with: STORY-384 (`aida queue recover` — the wizard this
  surface points at), BUG-270 (auto-bump format coverage), BUG-285
  (queue-done gate hole).
