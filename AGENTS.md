# AGENTS.md

Guidance for Codex and MCP-compatible coding agents working in the AIDA
repository. Read this as instructions-to-self: coordinate through AIDA,
keep git and the spec store coherent, and leave durable traces for the
next agent.

## Project Orientation

AIDA is an agent-collaboration substrate: stable spec IDs, typed
requirement relationships, code-to-spec trace comments, an autonomous
queue/orchestrator, and an MCP server that exposes the same graph to
non-Claude agents. Use `OVERVIEW.md` for product and architecture
context, `CLAUDE.md` for the broad repository guide, and
`docs/agents/cross-agent-onboarding.md` for shared MCP operating
context, and `docs/agents/session-communication.md` for Claude/Codex/
Antigravity session communication semantics.

## Storage Model

The spec graph is git-canonical. The orphan `aida-store` branch stores
one YAML object per requirement; `.aida-store/` is the live worktree for
that branch; `.aida/cache.db` is a rebuildable read cache. Do not create
parallel requirement files. Use AIDA tools and MCP for durable
coordination state.

When using a sibling worktree, link the store worktree if the local CLI
needs direct git-canonical access:

```bash
ln -s /home/joe/ai/aida/.aida-store .aida-store
```

## Requirements Management

Before implementing, read the owning requirement:

```bash
aida show <SPEC-ID>
```

Prefer MCP tools for spec graph and coordination operations:
`show_requirement`, `list_requirements`, `claim_task`, `release_task`,
`file_finding`, `post_punt`, `list_briefs`, `read_brief`, `ack_brief`,
`add_comment`, and `add_relationship`. Use shell commands for build,
test, git inspection, and cross-surface verification.

When adding requirements through MCP, pass a valid lowercase `type`.
AIDA derives the canonical ID prefix from that type, for example
`type: "task"` produces `TASK-N`. Do not invent generic `SPEC-N` IDs.

## Daily-Use Commands

```bash
codex mcp add aida -- aida mcp-serve
codex --cd /home/joe/ai/aida
aida show <SPEC-ID>
aida list --status approved
aida queue work <SPEC-ID>
aida agent new codex --role implementer --spec <SPEC-ID>
aida agent new codex --role advisor --show-context
aida pr ship
aida integrate
aida integrate --run
aida brief list --for-agent <agent>
aida brief ack .aida/agent-briefs/<agent>/<brief>.md
aida --asciinema --cast-title "Demo" queue work --batch <name> --auto-complete
tests/test_mcp_stdio.sh --skip-agent-contract
tests/test_mcp_doc_consistency.sh
```

Use `aida brief list --for-agent <agent>` (where `<agent>` is `codex` or `antigravity`) when a master/advisor session says
there is a pickup brief. Briefs live under `.aida/agent-briefs/<agent>/`,
embed the target spec plus setup/trailer reminders, and are local
runtime state. After reading one, run `aida brief ack <path>` so the
default list stays focused on pending work. If MCP is available, prefer
the MCP trio `list_briefs({agent: "<agent>"})`, `read_brief({path})`, and
`ack_brief({path})` so pickup works without shelling out.

For per-client MCP setup and marketplace/distribution notes, see
`docs/agents/aida-mcp-install-matrix.md`. It records the current config
surface for Claude Code, Codex, Cursor, Windsurf, Continue, Cline, Copilot,
Devin, Sourcegraph/Amp, and adjacent clients.
For Antigravity's AIDA-aware footer/title setup, run
`aida statusline setup --client antigravity --install` (or omit `--install` to
print the settings fragment).
Before publishing AIDA through a marketplace or registry, run
`docs/security/marketplace-publication-checklist.md`.

Prefer `aida agent new <type>` for supervised agent launches. It
registers the spawned process, writes a point-in-time launch context to
`.aida/agents/context/`, and passes its path as `AIDA_AGENT_CONTEXT_FILE`.
The context includes role guidance, active lease/spec details, pending
brief paths with titles, and queue-head hints. Use `--show-context` to
print the snapshot before spawning, or `--no-context` when the operator
intentionally wants a bare launch. The snapshot is not live-updating; keep
polling briefs/MCP after startup.

Use `aida --asciinema <subcommand>` for first-class terminal capture when
you need demo, training, or audit material. By default casts are written
to `.aida/casts/` at the project root (falling back to `~/.aida/casts/` if
run outside a project) with Windows-safe timestamps; pass `--cast-out`
and `--cast-title` to control the path and title.

## Worktree And Session Discipline

Do implementation work in a sibling worktree:

```bash
git fetch origin main:refs/remotes/origin/main
git worktree add /home/joe/ai/aida-<spec> -b <branch> origin/main
cd /home/joe/ai/aida-<spec>
```

No `.aida-store` symlink is needed — a sibling worktree resolves the
canonical store at the main worktree automatically (BUG-331).

Claim the spec before editing and release the lease after shipping. Do
not edit another agent's dirty main worktree. If branch, lease, or
worktree state looks inconsistent, stop and surface it rather than
forcing git.

## Direct Assignment: Implement BUG/TASK-N

When the operator says "implement BUG-N / TASK-N" and there is no queued
brief, follow this path (the same one used for TASK-132 and BUG-406):

1. `aida show <SPEC>` — read the spec, acceptance criteria, and any owning plan.
2. If it is Draft and the operator explicitly assigned it, promote it: `aida edit <SPEC> --status approved`.
3. Start an isolated session: `aida session start --owns <SPEC> --role implementer --base origin/main`.
4. Work in the sibling worktree (no `.aida-store` symlink — see above).
5. Implement; add `// trace:<SPEC> | ai:codex` comments; run targeted tests + `cargo fmt --all -- --check`.
6. Commit `[AI:codex] type(scope): description (<SPEC>)`.
7. `aida pr ship` — watches CI, squash-merges, pulls, and auto-bumps the spec to Completed.
8. End the session; verify the spec reached Completed.
9. Architecture-class work → sketch first and wait for master sign-off (see Sketch-First Protocol).

## Code Traceability

When code implements a spec, add a trace comment in the touched code:

```rust
// trace:TASK-123 | ai:codex
```

Keep spec IDs in developer artifacts: commits, PR titles, trace
comments, plans, and spec comments. Do not leak internal IDs into
user-facing CLI output unless that output is explicitly
developer/operator-facing.

## Commit And PR Format

Use the Codex prefix and put every shipped spec in trailing parens:

```text
[AI:codex] fix(scope): concise description (TASK-123)
[AI:codex] docs(agents): Codex setup integration (STORY-417 TASK-485 TASK-484)
```

The trailing parens are load-bearing. The auto-bump scanner reads them
when the squash commit lands on main. If one PR closes multiple specs,
include every shipped spec ID in the same trailing parens group.

## Sketch-First Protocol

Before opening a PR for architecture-class changes, post a sketch on the
owning spec and wait for master sign-off. Architecture-class means file
formats, MCP tool contracts, orchestrator semantics, lease model,
cross-cutting lifecycle vocabulary, or discipline/memory changes.
Bounded tests, docs refreshes, and acceptance-criteria implementation do
not need a sketch unless they introduce a reusable harness or new
project convention.

## MCP Notes

Trust MCP `tools/list` for tool names and argument names. Current AIDA
MCP responses are text envelopes with descriptor-level output schemas;
parse defensively until `structuredContent` ships. `aida mcp-serve`
checks the on-disk `aida --version` after each handled request; when the
binary version/build SHA has changed, it flushes the current response and
self-respawns so the next MCP request hits the newer binary. If an agent
still appears to serve stale behavior, kill that agent's `aida mcp-serve`
process and let the MCP client respawn it.

Headless orchestrator reliability fixes have the same binary-staleness
risk. If a drain is already running an older `target/debug/aida` or
`target/release/aida`, newly merged headless gates such as the
AskUserQuestion denial will not apply until the binary is rebuilt and the
next drain is launched from that binary. Use `aida dev status` when the
runtime behavior contradicts current source.

For hook-level pause/abort/resume semantics, especially Claude Code
`PreToolUse` / `PostToolUse`, `continue: false`, `ask`, and `defer`, use
`docs/agents/session-communication.md`. Do not assume a later hook can ask
whether to continue after an earlier hook has halted the run.

## Known Codex Pitfalls

- PR-201 missed the trailing spec trailer in the squash subject; that
  incident is why trailing-parens discipline is non-optional.
- Read the `aida pr ship` arc before relying on the wrapper in a new
  environment: SPEC-410, BUG-339, BUG-344, and BUG-345 document subject
  repair, parser alignment, CI startup waiting, and stale-main-worktree
  handling.
- If an instruction from another session sounds inconsistent with branch
  contents, verify the PR contents and flag the mismatch.

<!-- AIDA-AUTOGEN-BEGIN -->
# AIDA Conventions

This file is the single source of truth for AIDA's coding conventions in
this project. CLAUDE.md imports it via `@.claude/AIDA.md`; AGENTS.md
inlines a copy inside auto-generated delimiters. Edit this file to change
the conventions for both.

## Requirements management

This project tracks requirements with [AIDA](https://github.com/joemooney/aida).
**Do not maintain a separate `REQUIREMENTS.md`** — the requirements DB is
canonical.

Requirements database: `cache.db` (SQLite — legacy mode; new projects use the distributed git-canonical store at `.aida-store/`).

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
   raw `git pull` Just Work: `.aida/discipline/git-sync-and-review.md`.
- **Reviewing a PR?** `aida review prompt --pr N` lifts each linked
   spec's `## Acceptance` into a review prompt. Needs `gh`/`glab` for
   `--pr` mode; write a `## Acceptance` section in every STORY/BUG so
   there's something to lift. Detail: same discipline doc.
<!-- AIDA-AUTOGEN-END -->
