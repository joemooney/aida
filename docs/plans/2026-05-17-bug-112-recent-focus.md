# BUG-112 — `aida session list`: replace stale INITIAL TOPIC with RECENT FOCUS

- **Date:** 2026-05-17
- **Specs:** BUG-112
- **Status:** In progress
- **Complexity:** Small

## Approach

`aida session list`'s `INITIAL TOPIC` column shows Claude Code's `aiTitle`,
which is fixed at conversation start and never updates. For a long-running
session the column drifts far from what the session is actually doing —
misleading signal for the "which session was I in, what was I doing?"
question. Implement the spec's recommended **Option A**: drop `INITIAL
TOPIC`, add a `RECENT FOCUS` column populated from AIDA's own per-session
tracking.

Data sources, in precedence order:

```
Claude session (.jsonl, keyed by UUID)
        │  join on manifest.claude_session_id
        ▼
SessionManifest (.aida/sessions/<lease-id>.manifest.toml)
        │  manifest.session_id == lease id
        ▼
SessionActivityLog (.aida/sessions/<lease-id>.activity.toml)  ← newest-first
        │
        ├─ entries[0].spec_id          → RECENT FOCUS (live signal)
        ├─ else manifest most-recent   → RECENT FOCUS (fallback)
        │    item (latest started_at,
        │    else highest position)
        └─ no manifest                 → "-"  (AIDA isn't tracking this session)
```

The activity log is the strongest signal — it records actual `aida show`
/ `aida edit --status` / `aida queue done` interactions, newest-first, so
`entries[0].spec_id` is the genuine current focus. The manifest's planned
items are the fallback when no activity has been logged yet.

## Decisions

- **Keep the `SPEC` column.** It stays the first-mentioned (launch) spec;
  `RECENT FOCUS` is the latest. Together they show spec *evolution* —
  exactly Option A's "CURRENT scope/spec evolution" intent.
- **Keep the `title` field on `SessionMeta`.** `aida queue work
  --list-sessions` (`format_session_line`) still uses it; only the
  `session list` table and the `resume` picker swap to `recent_focus`.
- **`-` for untracked sessions, not a title fallback.** A session AIDA
  never tracked honestly has no spec focus. `-` is *absent* signal, not
  *misleading* signal — the bug is misleading-ness. SPEC + AGE + WORKTREE
  still identify the row.
- **Criterion 3 (dialog fallback to "most recent comment authored by the
  session ID") is deferred.** `Comment` carries only `author: String`
  (resolved from node identity via `get_default_author()`), not a Claude
  session ID — the criterion is not implementable without first stamping
  the Claude session ID onto comments. Filed as a follow-up.

## Files (build order)

1. `aida-cli/src/main.rs` — add `session_log_recent_spec()` pub(crate)
   helper next to `load_session_activity` (reads the activity log, returns
   `entries[0].spec_id`).
2. `aida-cli/src/session.rs` —
   - add `recent_focus: Option<String>` to `SessionMeta`;
   - add `recent_focus_from()` (pure) + `fill_recent_focus()` (I/O);
   - call `fill_recent_focus` in `list()` (here + parent) and
     `pick_interactive`;
   - header `INITIAL TOPIC` → `RECENT FOCUS`, row `title` → `recent_focus`
     in `print_table_with_widths`; same swap in `pick_interactive` labels;
   - replace the TASK-236 staleness caveat note.

## Critical Files

- `aida-cli/src/session.rs` — `SessionMeta`, `list`, `print_table_with_widths`,
  `pick_interactive`, `parse_session_meta`.
- `aida-cli/src/main.rs` — `SessionActivityLog` / `load_session_activity`.
- `aida-cli/src/session_manifest.rs` — `SessionManifest` / `list_all`
  (read-only consumer, no change).

## Reusable helpers

- `crate::session_manifest::list_all` — all manifests on disk.
- `load_session_activity` — already loads the newest-first activity log.
- `crate::find_project_root` — same root-resolution `normalize_specs` uses.

## Risks + gotchas

- `.aida/sessions/` is a symlink to the parent project's dir — one shared
  manifest/activity set across all worktrees; `find_project_root()` from
  any worktree resolves to it.
- Manifests written before TASK-112 have `claude_session_id = None` — they
  simply don't join and the session shows `-`. Acceptable.
- Last table column is unbounded (`{}`), so no `TableWidths` change needed.

## Tests (named)

- `recent_focus_prefers_activity_log_over_manifest` — activity entry wins
  over manifest items.
- `recent_focus_falls_back_to_latest_manifest_item` — no activity → latest
  `started_at`, else highest `position`.
- `recent_focus_none_for_empty_manifest` — no items → `None`.

## Verification

```
cargo test -p aida-cli session
cargo build -p aida-cli
./target/debug/aida session list      # column reads RECENT FOCUS, shows BUG-112
```

## Followups

- Stamp the Claude session ID onto comments so `aida session list` can
  fall back to a dialog session's most recent comment (BUG-112 acceptance
  criterion 3, deferred — needs comment-author/session-id wiring).

## Related

- TASK-236 — the staleness caveat note this change makes obsolete.
- STORY-56 — session-local activity log (the primary data source).
- TASK-112 — `claude_session_id` on the manifest (the join key).
