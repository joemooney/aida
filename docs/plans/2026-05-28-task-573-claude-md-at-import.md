# Plan: TASK-573 — `@docs/aida/discipline/README.md` in scaffolded CLAUDE.md

Date: 2026-05-28
Specs: TASK-573
Status: Completed
Complexity: ~10 prod LOC, ~5 test LOC, 1 commit, risk low

## Approach

Replace the inlined "Discipline for AIDA-using sessions" pointer list in the
scaffolded `CLAUDE.md` with a stub heading + `@docs/aida/discipline/README.md`
import. Claude Code expands `@`-imports recursively at session start, so the
canonical pointer table in the pack's README becomes the single source of
truth. Removes a ~32-line duplicate-maintenance hazard from the scaffolder
without losing in-session discoverability.

```
   BEFORE                              AFTER
   ─────────                           ─────
   CLAUDE.md inlines 8 bullets ─┐      CLAUDE.md has @-import ──► Claude
   (~35 lines, drifts on        │                                 expands
    every README edit)          │                                 README at
                                ▼                                 session
   docs/aida/discipline/                                          start.
   README.md (canonical)                                          One source.
```

## Decisions

- **Stub heading retained, not removed**. The scaffolded `## Discipline for
  AIDA-using sessions` H2 stays — it gives the section a frame in CLAUDE.md
  scrollback even when the model resolves the import inline. **Rationale**:
  Heading is cheap (one line); removing it would leave a bare `@`-import
  line under `## Project overview` with no visual separation.
- **No README trim in this PR**. The discipline README (44 lines) keeps its
  "Why this exists" + "Companion memory pack" narrative. **Rationale**:
  Advisor-judged scope cut — the README is now user-owned per-project after
  `aida init`, trimming it would force a separate scaffold-status drift
  conversation. The scaffolded CLAUDE.md source shrinks ~80% regardless; if
  loaded-context delta later proves to matter, the followup (filed below)
  trims README to push the delta positive.
- **Test asserts the @-import, plus a negative on the old bullets**.
  **Rationale**: Lock in the contract that the inlined bullets stay out —
  a future refactor that re-inlines them silently would break the spec's
  intent without breaking the build otherwise.

## Files (in build-order)

### `aida-core/src/scaffolding/claude_md.rs` — replace inline pointers with @-import

- `fn discipline_section`: collapse the 35-line raw-string body to a 3-line
  stub (`## Discipline for AIDA-using sessions` heading + blank +
  `@docs/aida/discipline/README.md`). Doc comment gains a `trace:TASK-573`
  line referencing this change.
- `mod tests::generated_claude_md_has_discipline_section`: replace the
  bullet-content asserts (`advisor-role.md`, `machinery-glossary.md`,
  `tag-conventions.md` substrings) with two new asserts — positive on
  `@docs/aida/discipline/README.md`, negative on `- **Roles**` and
  `- **Start here**` (to lock the inlined-bullets regression out).

## Critical Files

- `aida-core/src/scaffolding/claude_md.rs`

## Reusable helpers (do not reimplement)

- `Scaffolder::generate_claude_md` (`aida-core/src/scaffolding/claude_md.rs`)
  — the canonical scaffolder this change plugs into; no new entry points.
- `CLAUDE_AIDA_IMPORT` constant + `claude_md_has_import` / `insert_claude_md_import`
  (same file) — the existing @-import pattern (for `.claude/AIDA.md`) is
  the template the discipline change mirrors.

## Risks + gotchas

1. **Risk**: Claude Code's `@`-import has nuances — e.g., it may not resolve
   relative paths when CLAUDE.md is read from a non-project-root CWD.
   **Mitigation**: The path is project-relative (`docs/aida/discipline/README.md`),
   which is the documented pattern. Same shape as the existing
   `@.claude/AIDA.md` import, which has shipped since FR-1-035 without
   reports of resolution failures.
2. **Risk**: Existing projects that already ran `aida init` keep the
   inlined bullets in their CLAUDE.md (since the file is user-owned post-
   scaffold). **Mitigation**: Accepted. `aida init --force` re-emits the
   lean version; on a routine refresh the user's edits to CLAUDE.md should
   win — re-flowing inlined bullets out is not load-bearing.
3. **Risk**: Loaded-context tokens may go up slightly (README is 44 lines
   vs the prior 35-line inline). **Mitigation**: Documented in
   "Measurement" below — source shrinks ~80%, loaded context grows ~9
   lines net. Followup TASK trims the README if it matters.

## Tests (named, not "add tests")

- `generated_claude_md_has_discipline_section` — updated. Positive asserts
  on heading + `@docs/aida/discipline/README.md`; negative asserts on the
  removed `- **Roles**` and `- **Start here**` bullets.

## Verification

Executable smoke against the worktree-local release binary:

```bash
WORKTREE=/home/joe/ai/aida-task-573
TMP=$(mktemp -d); cd "$TMP" && git init -q && \
  git config user.email "smoke@test.local" && \
  git config user.name "smoke"
"$WORKTREE/target/release/aida" init >/dev/null 2>&1
grep -q '^@docs/aida/discipline/README.md$' CLAUDE.md   # positive
! grep -q '^- \*\*Roles\*\*' CLAUDE.md                  # negative
test -f docs/aida/discipline/README.md                  # import target exists
wc -l CLAUDE.md                                          # expect ~18 lines
```

**Worktree-aware binary path** (TASK-388): this worktree has a
local `target/` (`/home/joe/ai/aida-task-573/target/release/aida`) created
by `cargo build --release` from the worktree. Verification uses the
absolute path under `$WORKTREE/target/release/`.

## Measurement: before/after

Acceptance criterion 4 — *Net token reduction documented*. Measured by
running `aida init` from the unmodified binary vs the rebuilt binary
against a throwaway `/tmp/aida-task-573-smoke` project.

| Surface                                      | Before              | After             | Delta            |
| -------------------------------------------- | ------------------- | ----------------- | ---------------- |
| Scaffolded `CLAUDE.md` source                | 47 lines / 2,262 B  | 18 lines / 433 B  | −29 lines / −81% |
| Inline discipline pointer content            | ~35 lines / ~1,650 B | 1 `@`-import line | −34 lines        |
| `docs/aida/discipline/README.md` (unchanged) | 44 lines / 3,485 B  | 44 lines / 3,485 B | 0                |
| Net loaded-context (CLAUDE.md + imported README) | ~47 lines | ~58 lines       | +11 lines        |

**Net loaded-context delta is small and slightly positive (~+11 lines),
not the desired negative.** The README (44 lines) is slightly fatter than
the prior inlined 35-line section because it carries narrative metadata
("Why this exists", "Companion memory pack"). The dominant win is
**−81% reduction in scaffolded CLAUDE.md source** — eliminates the
duplicate-maintenance hazard between `discipline_section()` and the README.
If the loaded-context delta turns out to matter, the README trim follows
as a separate PR (see Followups).

## Followups

- File a TASK: `README: trim narrative metadata (Why this exists, Companion
  memory pack) to push loaded-context delta negative`. Tag
  `batch:scaffolding-2026-05-28`, `aida:init`, `context-budget`.
  **Trigger to file**: defer until evidence emerges that the +11-line
  loaded delta meaningfully costs context — the substrate gain (single
  source of truth) is the keystone win regardless.

## Related

- FR-1-035 — established the `@.claude/AIDA.md` import pattern (the
  template this change mirrors).
- STORY-255 — original discipline pack scaffolding (introduced the inlined
  `discipline_section`).
- STORY-443 — pack relocation under `docs/aida/` namespace.
- TASK-338 — added the machinery-glossary bullet (now obsoleted by the
  README-as-source-of-truth model).
