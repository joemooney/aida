<!-- AIDA Generated: v2.0.0 | checksum:b81301bd | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->

# AIDA Conventions

This file is the single source of truth for AIDA's coding conventions in
this project. CLAUDE.md imports it via `@.claude/AIDA.md`; AGENTS.md
inlines a copy inside auto-generated delimiters. Edit this file to change
the conventions for both.

## Requirements management

This project tracks requirements with [AIDA](https://github.com/joemooney/aida).
**Do not maintain a separate `REQUIREMENTS.md`** — the requirements DB is
canonical.

Requirements database: distributed git-canonical store at `.aida-store/` (orphan branch `aida-store`, plus a rebuildable SQLite cache at `.aida/cache.db`).

### Daily commands

```bash
aida list                              # list all requirements (cache-backed)
aida list --status draft               # filter by status
aida show <ID>                         # show details (e.g. `aida show FR-0042`)
aida search "<query>"                  # full-text search
aida add --title "..." --type <type> --status draft
aida edit <ID> --status in-progress
aida edit <ID> --status completed
aida comment add <ID> "implementation note..."
aida rel add --from <ID> --to <ID> --type <Parent|Verifies|References>
aida history                           # what was touched recently (digest)
aida statusline                        # one-line: project · role · queue · cache
```

### Requirement-first development

1. **Before coding:** check whether the work has a SPEC-ID. If not, create one
   (`aida add --type <task|story|bug|...> --status approved --title "..."`).
2. **During coding:** add inline trace comments referencing the SPEC-ID.
3. **Before committing:** mark the requirement `completed` (or `in-progress`
   if work continues), and ensure the commit message references it.

## Inline trace comments

Add a comment near the code that implements (or fixes, or verifies) a
requirement:

```rust
// trace:FR-0042 | ai:claude
fn implement_feature() { /* ... */ }
```

Format: `// trace:<SPEC-ID> | ai:<tool>[:<confidence>]`

- `<SPEC-ID>` — e.g. `FR-0042`, `BUG-1-017`, `TASK-0344`
- `<tool>` — `claude`, `codex`, `copilot`, `human`, `aider`, …
- `<confidence>` — optional: `high` (implied), `med` (40-80% AI), `low` (<40% AI)

## Commit message format

```
[AI:tool] type(scope): description (REQ-ID)
```

Examples:

```
[AI:claude] feat(auth): add login validation (FR-0042)
[AI:claude:med] fix(api): handle null response (BUG-0023)
[AI:antigravity+claude] test(hooks): accept mixed authorship (TASK-509)
chore(deps): update dependencies        # no REQ-ID needed
docs: update README                     # no REQ-ID needed
```

Rules:

- `[AI:tool]` required when commit includes AI-assisted code (any file with a
   `// trace:... | ai:tool` comment changed). Use `[AI:tool1+tool2]` for
   mixed-agent authorship, with optional confidence on the whole commit
   (`[AI:tool1+tool2:med]`).
- `type` required: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`,
   `build`, `ci`, `chore`, `revert`.
- `(scope)` optional — component or area affected.
- `(REQ-ID)` required for `feat`/`fix`; optional for `chore`/`docs`.

Set `AIDA_COMMIT_STRICT=true` (or commit through the `/aida-commit` skill) to
enforce; otherwise the commit-msg hook just warns on non-conforming messages.

## Capture proactively, not reactively

The requirements DB is only valuable when it stays in sync with reality.
Treat `/aida-capture` as a habit, not a safety net:

1. **Spec-first when introducing a new theme.** New command, new field on a
   core model, new skill, new architectural pattern — pause and `aida add`
   *before* the implementation commits. ~2 min cost; saves backfill later.
2. **Don't reuse one EPIC as a catchall.** When the work has drifted from
   what the EPIC was originally about, that's a signal to create a new EPIC,
   not stretch the existing one.
3. **Run `/aida-capture` at natural pauses.** End of focused work, before
   compaction, when stepping away. Five-minute pass that catches missed reqs.
4. **Yellow flag at >5 untracked commits.** Five+ feat/fix commits without a
   matching requirement → offer to capture before continuing.
5. **Trace comments must match reality.** A `// trace:EPIC-N` on code that
   has nothing to do with EPIC-N is misinformation that compounds. If you're
   unsure which spec a piece of work belongs to, that's the signal it needs
   its own.

## Glance at the statusbar

`.claude/settings.json` wires `aida statusline` into Claude Code's status
bar. It shows project · active role · queue depth · cache freshness. If the
role you expect isn't there, you forgot to `aida role enter <name>` before
starting the session.

## Git sync & review workflow

- **`aida pull` refusing (divergent branches)?** The code leg is
   `git pull --ff-only` (won't auto-rebase your tree); the store leg
   is `--rebase`. Recovery recipe + the one-time `git config` to make
   raw `git pull` Just Work: `docs/aida/discipline/git-sync-and-review.md`.
- **Reviewing a PR?** `aida review prompt --pr N` lifts each linked
   spec's `## Acceptance` into a review prompt. Needs `gh`/`glab` for
   `--pr` mode; write a `## Acceptance` section in every STORY/BUG so
   there's something to lift. Detail: same discipline doc.

## Claude Code skills (slash commands)

This project ships a curated set of `/aida-*` skills under `.claude/skills/`,
each with a matching slash command in `.claude/commands/`. Daily drivers:

- `/aida-req` — add a new requirement with AI evaluation
- `/aida-implement` — implement a requirement with trace comments + status updates
- `/aida-plan` — decompose a requirement into an implementation plan
- `/aida-evaluate` — score a requirement on clarity / testability / completeness
- `/aida-capture` — review the current session and capture missed requirements
- `/aida-commit` — commit with automatic requirement linking
- `/aida-pickup` — peek at the next item routed to your active role and start work
- `/aida-queue` — read-only queue inspection (counterpart to `/aida-pickup`)
- `/aida-search` — unified search across requirements + code

Run `ls .claude/skills/` for the full skill catalog.

### Per-project skill extensions (STORY-305)

Two mechanisms let a project extend skills without forking them.
AIDA never overwrites either, and `make sync-templates` never
touches them:

1. **Project-owned new skills** live under
   `.claude/skills/local/<my-skill>.md`. Claude Code discovers
   them the same way it discovers stock skills.
2. **Stock-skill extensions** live alongside the stock skill as
   `.claude/skills/<name>.local.md`. When `/aida-<name>` is
   invoked, treat the stock skill as the base and **append**
   the `<name>.local.md` content as project-specific guidance
   with last-word authority (later instructions override
   earlier ones — normal markdown precedence).

**For Claude Code**: whenever you load a stock skill from
`.claude/skills/<name>.md`, also read `.claude/skills/<name>.local.md`
if it exists in the same directory and treat its contents as
appended to the stock skill. Both files together are the
effective skill. See `docs/extending-skills.md` for examples.
