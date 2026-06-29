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

Files this work will *create* don't exist yet — mark them with `(new)` (or
`(to create)`) right after the backticked path, e.g. `` `src/widget.rs` (new) ``.
`aida plan verify` then accepts the missing file instead of erroring, and
warns if a `(new)`-marked file already exists (a stale-plan signal).

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

**Worktree-aware binary path** (TASK-388). When a verification recipe
needs to invoke the built `aida` binary directly (not via PATH), do NOT
use bare `target/debug/aida` — AIDA's cargo setup puts build output at
the **main repo's** `target/`, so a worktree's CWD has no local `target/`
and the relative path resolves to nothing. Use one of:

```bash
# Option A — absolute path from anywhere in this repo (recommended)
AIDA_BIN="$(git rev-parse --show-toplevel)/target/debug/aida"

# Option B — explicit absolute path (clearer in copy-paste recipes)
AIDA_BIN=/home/<user>/path/to/aida/target/debug/aida

# Option C — an activated dev shell (`aida dev activate`) already on PATH
AIDA_BIN=aida
```

The bare `target/debug/aida` form has bitten verification recipes from
worktrees enough times to file a spec; pick A or C as the default.

## Followups

Out-of-scope items the implementer should NOT do now. Lighter than child
requirements (which block) — these are post-merge cleanup. `aida queue done`
parses this section and offers to file each bullet below as a child TASK
(the auto-bump does the same on merge), so keep each bullet to one
TASK-title-sized line.

- Reverted-commit handling.
- Statusline color for new state.

## Related

- Builds on: STORY-X
- Blocks: BUG-Y
- See also: docs/positioning/<comparison>.md
