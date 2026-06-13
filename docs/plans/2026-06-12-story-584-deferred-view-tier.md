# STORY-584 — `deferred`: a third view-state between active and archived

- **Date:** 2026-06-12
- **Specs:** STORY-584 (parent), TASK-773 (split-out child: standing-artifact type exclusion)
- **Status:** Implemented
- **Complexity:** Medium (clones the STORY-441 `archived` plumbing end-to-end + one new field)

## Approach

Add a `deferred` view-flag **parallel to `archived`** — a boolean orthogonal to `status`, not a
new lifecycle state. The three tiers become **active (default) / deferred / archived**. The one
thing distinguishing deferred from archived is a free-text **revisit trigger** (`deferred_until`):
deferred = prospective/primed ("returns when X"), archived = retrospective/filed.

The implementation mirrors the `archived` axis at every layer (model → YAML serde → SQLite cache
schema + filter → backend → CLI verbs + list/search/history flags → MCP projection → export/import
→ TS types), and adds a second column the archive axis doesn't have: the revisit trigger.

```
aida defer <id> --until "<cond>"   → sets deferred=true, deferred_at=now, deferred_until=<cond>
aida list                          → DeferFilter::NonDeferredOnly  (hides flag OR deferred:* tag)
aida list --deferred               → DeferFilter::DeferredOnly + "Revisit triggers:" section
aida list --all                    → DeferFilter::Both (union of all 3 tiers)
aida undefer <id>                  → clears flag + trigger
```

## Decisions

1. **Reuse archived's flag plumbing, don't touch the state machine.** `deferred`/`deferred_at`
   added next to `archived`/`archived_at`; defer carries **no terminal-status guard** (archive's
   guard exists because archive is for the closed long-tail — defer is explicitly for shelving
   *live, open* backlog).
2. **Honor-both migration (criterion 5, operator-confirmed).** The default view hides a spec if the
   `deferred` flag is set **OR** it carries any legacy `deferred:*` parking tag (SQL:
   `tags_json LIKE '%"deferred:%'`). Bridges the ~14 existing `deferred:*`-tagged specs with no bulk
   re-defer. The burndown/queue pickability gate already honored those tags; now the list view agrees.
3. **`--all` widens both axes; the only-X views are mutually exclusive.** `--archived`/`--deferred`/
   `--all` conflict (`conflicts_with_all`). `--archived` keeps the defer axis open (audit
   completeness); `--deferred` keeps `archive=NonArchivedOnly` (deferred-but-not-archived).
4. **Explicit `deferred:`-tag filter opens the axis.** `aida list --tags 'deferred:*'` would
   otherwise contradict the default hide and return nothing; detected and bumped to `Both`.
5. **Criterion 6 split to TASK-773 (operator-confirmed).** Excluding standing-artifact types
   (vision/principle/term/constraint/folder/meta) from `aida list open` is type-based filtering,
   orthogonal to the view-flag clone — kept out to keep this slice clean.
6. **MCP parity = archived's parity.** The archive axis isn't exposed on MCP list/search tools, so
   deferred isn't either (the MCP projection still carries the fields).

## Files (build order)

- `aida-core/src/models.rs` — `deferred` / `deferred_at` / `deferred_until` fields + Default.
- `aida-core/src/db/cache_schema.sql` — 3 columns + 2 indexes.
- `aida-core/src/db/cache.rs` — `DeferFilter` enum, `DEFER_TAG_LIKE`, `ListFilter.defer`,
  `RequirementSummary` fields, INSERT/SELECT/row-map (column index shift), `search()` + clause,
  SCHEMA_VERSION 3→4, 6 new tests.
- `aida-core/src/db/{cached_git_backend,mod}.rs`, `aida-core/src/lib.rs` — re-export + `search()` arg.
- `aida-core/src/db/{postgres,sqlite}_backend.rs` — construct new fields (not persisted by legacy/pg).
- `aida-core/src/export.rs` — export/import `deferred` + `deferred_until`.
- `aida-cli/src/cli.rs` — `Defer`/`Undefer` commands; `--deferred` on list/search/history;
  `--all`/`--archived`/`--deferred` mutual exclusion via `conflicts_with_all`.
- `aida-cli/src/main.rs` — `defer_single` / `handle_undefer_command`, dispatch, list/search/history
  DeferFilter wiring, `print_deferred_triggers` + `print_hidden_hints`, per-write auto-push list,
  deprecated-backend bail. (Also fix-forward: research-lane `spawn_claude_headless` 4→5 args —
  pre-existing break from the STORY-567/STORY-568 merge race.)
- `aida-cli/src/history.rs` — `deferred_specs` / `deferred_only_specs` opts + filters + footers.
- `aida-cli/src/{mcp,findings,rules_sync,backlog}.rs` — new fields in summary/req constructions.
- `shared/types.ts` — regenerated (`cargo run -p aida-generate-types`).
- `CLAUDE.md` — daily-use commands + "deferred ≠ status, deferred ≠ archived" lifecycle note.

## Tests

- `aida-core` cache unit tests (6 new): non-deferred-only excludes; honor-legacy-tag; deferred-only
  returns flag+tag; both returns everything; defer/archive axes compose; fields round-trip.
- Full suites green: aida-core 439, aida-cli 2361, MCP stdio suite, fmt `--check`, clippy (no
  correctness lints / no new warnings).
- Manual E2E on a throwaway spec: defer `--until` → re-defer preserves trigger → shows in
  `--deferred` with trigger → hidden from default → undefer → undefer-again errors. Cleaned up.

## Verification

```bash
cargo test -p aida-core db::cache
cargo test -p aida-cli --bin aida
bash tests/test_mcp_stdio.sh
aida list --deferred            # honors deferred:* tags; shows Revisit triggers
aida list                      # nudges "(N deferred hidden — pass --all or --deferred …)"
```

## Followups

- TASK-773: exclude standing-artifact types from `aida list open` (criterion 6).
- (Maybe) surface deferred state + trigger in `aida show` — deferred only when archive is too (parity).
- (Maybe) an age-based `aida defer --older-than` bulk sweep, if demand appears (archive has one).

## Related

- STORY-441 (the archive view-flag this clones), the pickability gate (already honors `deferred:*`),
  `docs/aida/discipline/machinery-glossary.md` lifecycle vocabulary.
