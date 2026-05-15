# Plan: <STORY-N or short title>

Date: YYYY-MM-DD
Specs: STORY-N, BUG-M
Status: Draft | Approved | In Progress | Completed
Complexity: ~N prod LOC, ~M test LOC, K commits, risk low|medium|high

<!--
  AIDA plan template. Save plans as docs/plans/YYYY-MM-DD-<slug>.md.
  Prefer SYMBOL refs (`fn handle_pull_command`) over LINE refs (`main.rs:19713`):
  symbol refs survive edits, line refs drift fast. trace:TASK-92
-->

## Approach

One paragraph executive summary stating the whole strategy. A reader should be
able to stop here and still know what's happening.

### Diagram (optional but high-value)

ASCII state machine, swimlane, or call-flow. Compresses paragraphs of prose
into ~10 lines that survive skimming. Worked example: the lifecycle and
auto-bump call flow in `docs/plans/2026-05-13-story-86-done-status.md`.

```
   InProgress ──► Done ──pull on default branch──► Completed
                   ▲
                   └── aida queue done
```

## Decisions

Each significant choice resolved with rationale. NOT an open-question list —
these are the calls being made.

- **Decision X**: <chosen option>. **Rationale**: <why over alternatives>.
- **Decision Y**: <chosen option>. **Rationale**: <why over alternatives>.

## Files (in build-order)

Symbol-anchored where possible. Order matters — top-to-bottom so each commit
builds clean.

### `path/to/file.rs` — purpose

- `fn foo`: <specific edit shape>
- `struct Bar`: add field `baz: Option<T>`

### `path/to/other.rs` — purpose

- `fn bar`: <specific edit shape>

## Critical Files

Flat enumeration of must-touch paths, separate from prose. At-a-glance blast
radius. Should match what the **Files** section enumerates.

- `path/to/file.rs`
- `path/to/other.rs`
- `aida-core/templates/skills/...md`

## Reusable helpers (do not reimplement)

Explicit list of existing helpers the implementer should call rather than
re-invent. The single highest-leverage section — saves the implementer from
duplicating what's already in the codebase.

- `extract_spec_ids_from_commit` (`aida-cli/src/main.rs`) — parses (REQ-ID) from commit subjects.
- `git_ops::head_sha`, `git_ops::current_branch` — git introspection.
- `Storage::update_atomically` — works across SQLite + git-canonical backends.

## Risks + gotchas

Numbered, each with mitigation. The adversarial-thinking pass.

1. **Risk**: <forward compat / migration / perf / race condition>. **Mitigation**: <approach>.
2. **Risk**: <edge case>. **Mitigation**: <approach>.

## Tests (named, not "add tests")

- `auto_bump_done_to_completed_picks_up_subject_refs` — happy path.
- `auto_bump_skips_when_not_on_default_branch` — negative case.
- `queue_done_flips_to_done_not_completed` — invariant.

## Verification

Executable bash smoke (positive + negative). End-to-end definition of done.

```bash
TMP=$(mktemp -d); cd "$TMP" && git init && aida init
aida add --title "smoke" --type story --status approved
# ... drive the lifecycle, assert end-state
aida show STORY-X | grep -i status     # expect: Completed
```

## Followups (file as TASKs at completion time)

Out-of-scope items the implementer should NOT do now but should file at
queue-done time. Lighter than child requirements (which block) — these are
post-merge cleanup.

- Reverted-commit handling.
- Statusline color for new state.

## Related

- Builds on: STORY-X
- Blocks: BUG-Y
- See also: docs/positioning/<comparison>.md
