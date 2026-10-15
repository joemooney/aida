# 2026-10 documentation cleanup record

<!-- trace:TASK-1611 | ai:codex+claude -->

This record documents the October 2026 documentation audit: what changed in
the repository between August and October, the archive policy, and the
disposition of every tracked document. [`inventory.tsv`](inventory.tsv)
holds the per-file inventory, and [`inventory.py`](inventory.py) regenerates
it from git and checks its coverage.

## How the cleanup was done

| Step | Commit | What |
|------|--------|------|
| Audit base | `738653fb3e` | `main` when the audit began (2026-10-07). |
| First pass (Codex) | `5b11a4a7d2` | 106 moves into `docs/archive/<category>/` and `docs/release-notes/`, plus mechanical retargeting of paths that named the moved files. Committed exactly as that session left the worktree. |
| Merge | `251563b616` | Normal merge of `main` at `b46aec1e67`; no conflicts. |
| Completion (Claude) | the commit that adds this file | Restored six files that are still consumed in place, rebuilt the path retargeting from current `main` under the policy below, and added this record, the inventory, and the navigation updates. |

The pre-cleanup text of any file is available with
`git show 738653fb3e:<path_before_cleanup>`, using the inventory's
`path_before_cleanup` column.

## Evolution evidence, 2026-08-01 to 2026-10-07

Counts come from `git log 738653fb3e` restricted to that window:

| Measure | Value |
|---------|-------|
| Commits | 810 (August 28, September 643, October 1–7 139; calendar months by recorded author date) |
| Commits touching `*.md` or `docs/` | 336 |
| Markdown files added | 167, mostly skill mirrors (`.claude/skills`, 61), plans (23), template masters (23), and discipline-pack mirrors (14) |
| Most frequent commit scopes | integrate (108), drain (51), queue (45), orchestrator (29), review (27), cli (20) |
| Commit types by agent | Codex: fix 279, feat 101, docs 21. Claude: fix 105, chore 105, feat 42, docs 17. Antigravity: fix 13, feat 7. |

Over these weeks, work concentrated on the control plane: integration,
drain, queue, orchestrator, and review. Documentation followed that work
into `docs/cli/`, the discipline pack, and dated plans and spikes. Most of
the May–July positioning, competitive-analysis, research, and brief
documents saw few or no edits in this window (see each file's
`last_commit_before_audit`). They describe the product as it stood when
they were written, which is why they moved to the archive.

## Archive policy

1. **Archive** dated snapshots, one-off audits, handoff briefs, superseded
   design notes, and rendered exports whose source of truth lives
   elsewhere. Each moves to `docs/archive/<category>/` under its original
   file name. The one exception is the undated `docs/flow-smoke.md`, which
   became `reviews/2026-07-flow-smoke.md`.
2. **Keep in place** anything a build, test, tool, or scaffold reads at its
   path. That covers template masters and their generated mirrors (`.claude/`,
   `.aida/discipline/`, scaffolded `docs/agents/` files), normative ADRs in
   `docs/aida/05-decisions/`, live agent instructions (`AGENTS.md`,
   `CLAUDE.md`, generated `REVIEW.md`), and dated plans in `docs/plans/`
   (the plan convention).
3. **Historical records stay historical.** Archived files, plans, research,
   spikes, dated decks, release notes, and `CHANGELOG.md` keep their prose
   verbatim, even where it is obsolete or names a path that has since moved.
4. **Links follow the file.** Every Markdown or HTML link target that
   resolved before the move was retargeted to the file's new location, in
   every file, including historical ones, so links keep working.
   Directory links follow wholly moved directories.
5. **Maintained documents** (entry points, guides, agent docs, templates)
   also had backtick path mentions updated, so readers are pointed at the
   current location. Navigation (`docs/README.md`, the positioning and
   competitive-analysis indexes, the root `README.md`, and `OVERVIEW.md`)
   says where the archived material now lives and no longer calls an
   archived snapshot "current".

## Dispositions

| Disposition | Files |
|-------------|------:|
| archived | 98 |
| renamed (`whats-new-YYYY-MM.md` → `docs/release-notes/YYYY-MM.md`) | 2 |
| restored (moved by the first pass, returned) | 6 |
| added (this record) | 1 |
| kept | 749 |

### Restored files

The first pass archived these six files, but each is still read at its
original path, so they were returned:

| File | Reason |
|------|--------|
| `docs/cli-format-json-audit.md` | `include_str!` in `aida-cli/tests/bug_1502_format_json_audit.rs`; moving it breaks the test build. |
| `REVIEW.md` | Generated review instructions that managed Code Review reads from the repository root (`aida review assemble`). |
| `docs/agents/codex-mcp-roundtrip-verdict.md` | Scaffolded by `aida init` from its template master. |
| `docs/agents/claude-surfaces-codex-parity.md` | In-repository copy of a template master. |
| `docs/user-guide.html`, `docs/user-guide-dark.html` | Opened by `aida user-guide [--dark]`; written by `helper/generate-docs.sh`. |

The first pass's text substitution also changed prose outside links. For
example, it replaced every bare `REVIEW.md` (including inside
`INTEGRATION_REVIEW.md`), and paths quoted in `CHANGELOG.md` entries and
commit subjects. The completion commit rebuilt all retargeting from current
`main` under the policy above, so those history rewrites are gone.
`CHANGELOG.md` is unchanged from `main`.

## Known residual references

These name an old path in prose or code comments. None is a link, and
none is read at run time:

- 59 backtick path mentions inside historical records (policy rule 3).
  Resolve them with the inventory's `path_before_cleanup` column.
- Code comments: `aida-cli-lib/src/lib.rs` cites
  `docs/spikes/2026-06-07-spike-8-ultraplan-comparison.md` and
  `docs/research/2026-06-29-warm-pool-build-delta.md` (now under
  `docs/archive/spikes/` and `docs/archive/research/`).
  `.aida/discipline/glossary.yaml` and its template master cite
  `docs/competitive-analysis/2026-05-26-agent-memory-libraries.md` (now
  under `docs/archive/competitive-analysis/`). These were left unchanged to
  keep the cleanup to documentation files.
- `aida-cli-lib/src/tests/bug708_under_specified_tests.rs` names
  `docs/flow-smoke.md` inside fixture spec text; it never opens the file.
- `machinery-glossary.md` (template master and its `.aida/discipline/`
  mirror) links to `../archive/positioning/vs-claude-code-subagents.md`.
  The link is written relative to the generated `docs/cli/12-glossary.md`,
  where it resolves; like the `../positioning/` link it replaces, it does
  not resolve from the discipline-pack location.

## Verification

- Inventory coverage: `python3 docs/archive/2026-10-cleanup/inventory.py --check`
  confirms every file moved relative to the merged `main` appears with its
  correct origin, and that every row names a tracked file.
- Link proof: a resolver over `git archive` exports of `main` (`b46aec1e67`)
  and of the cleanup tree compared every Markdown link, reference definition,
  HTML `href`/`src`, backtick repository path, and docs path string literal
  in code. The cleanup introduces no unresolved link, reference definition,
  or `href`/`src`, and it fixes one link that was already broken on `main`
  (`writeups/` in `docs/README.md`). The only new unresolved items are the
  residual references listed above, plus this record's own backtick
  mentions of old paths.

## Not committed

The first pass also saved copies of nine entry points under `before/` in this
directory. They are byte-identical to `738653fb3e`, and their relative links
do not resolve from this location, so they stay out of version control. Use
`git show 738653fb3e:<path>` instead.
