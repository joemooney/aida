# CLAUDE.md

Guidance for Claude Code working in the AIDA repository. Project background and high-level architecture live in `OVERVIEW.md`; this file focuses on conventions and reference material you need while writing code here.

## Project orientation

AIDA = AI Design Assistant. The defensible niche is the **agent-collaboration layer**: stable spec IDs, typed relationships, code-to-spec trace comments, and an MCP server that exposes the requirement graph to coding agents. Karpathy-style "structured markdown queryable by Claude" is the floor; AIDA adds the relationship graph + identifier stability + enforcement loop.

For the full vision, architecture, and surface inventory see `OVERVIEW.md`. For the path-forward audit and current direction see `docs/plans/2026-05-02-git-canonical-storage.md`. For the *"should I use AIDA or X?"* question in any of its forms see `docs/positioning/` (one focused comparison per neighbor tool: `vs-ultrareview.md`, `vs-karpathy-md.md`, `vs-saas-pm.md`).

**Workspace** (5 crates): `aida-core` (engine), `aida-cli` (`aida` binary + MCP server), `aida-crate` (published `aida` crate metadata), `aida-server` (REST + gRPC, port 8080), `aida-generate-types` (Rust → TypeScript). React dashboard at `aida-web-react/` (port 5173 dev). Native desktop and WASM clients were extracted to a separate repo on 2026-05-02.

## Storage model (EPIC-1-001)

**Git-canonical by default.** The orphan `aida-store` branch is the writer of record (one YAML file per requirement under `objects/TYPE/000/SPEC-ID.yaml`). A SQLite cache at `.aida/cache.db` (gitignored, auto-rebuilt) is a rebuildable read projection used to make list/filter/search fast without scanning hundreds of YAML files. Writes go to git first, then the cache (write-through). Stale-detection compares the cache's recorded HEAD SHA against the orphan branch's actual HEAD; mismatch triggers rebuild on next read.

- Live worktree: `.aida-store/` (gitignored)
- Branch: `aida-store` on origin
- Cache: `.aida/cache.db` (gitignored)
- Manage cache: `aida cache status`, `aida cache rebuild`

**`.gitignore` convention for `.aida/`:** deny-by-default. The `.gitignore` block scaffolded by `aida init` is `.aida/*` plus an explicit `!.aida/config.toml` allow-list. Anything new under `.aida/` is runtime per-clone state by convention; tracking a new project-config file requires adding a `!.aida/<name>` line. This avoids the recurring "new feature wrote a new runtime file, session-end refuses on the untracked path" papercut. trace:BUG-73 | ai:claude

PostgreSQL is opt-in via the `postgres` feature flag. Legacy standalone YAML/SQLite backends still exist for the deprecated `aida init --centralized` opt-in path; they print a deprecation warning at init time. **Don't** add new code paths that use them.

Architecture: `aida-core/src/hlc.rs`, `dispenser.rs`, `node.rs`, `object_store.rs`, `db/git_backend.rs`, `db/cache.rs`, `db/cached_git_backend.rs`, `git_ops.rs`, `conflict.rs`.

## Requirements management

This project uses AIDA for its own requirements tracking. **Do NOT maintain a separate `REQUIREMENTS.md` file** — the orphan-branch YAML files plus the cache are the source of truth. Use `aida list`, `aida search`, `aida show <ID>`, `aida add`, `aida edit`, `aida comment add` to work with them.

### Project initialization (for new projects, not this repo)

```bash
aida init                      # Default: distributed git-canonical (RECOMMENDED)
aida init --sibling            # Distributed using a sibling repo (multi-repo workspaces)
aida init --centralized        # Legacy SQLite mode (deprecated, prints warning)
aida init --no-skills          # Skip .claude/skills/ and .claude/commands/
aida init --no-hooks           # Skip .claude/hooks/ and git hooks
aida init --force              # Overwrite existing files
```

`aida init` creates: orphan branch `aida-store` + worktree at `.aida-store/`, `.aida/config.toml`, `.aida/cache.db`, META requirements seeded into the orphan store, `.mcp.json`, `CLAUDE.md`, `AGENTS.md` (Codex), `.claude/skills/` + `commands/` + `hooks/`, `docs/plans/`.

### Daily-use commands

```bash
aida list                              # Cache-backed (sub-ms vs full-store load)
aida list --status draft               # Filter by status
aida search "<query>"                  # Cache-backed FTS5 search
aida show <ID>                         # Show requirement details (FR-1-001 or FR-1)
aida add --title "..." --type story --status draft --tags "tag1,tag2"
aida edit <ID> --status completed
aida comment add <ID> "..."
aida db merge-gate                     # Assign agreed short IDs (FR-7-001 → FR-1)
aida db sync --pull --push             # Sync orphan branch with remote
aida cache status                      # Compare cache HEAD vs git HEAD
```

### Queue identity (BUG-89)

The queue's `user_id` is the **shell's** user identity — not the node identity from `~/.aida/node.toml`, not the email in `[node]`, not the role's stored `user_id`. Every queue path (`add`, `list`, `next`, `done`, `remove`, `move`, the role-show queue head, the statusline depth) routes through `current_user_id()` in `aida-cli`, which resolves in order: `--user <id>` flag → `AIDA_USER` env → `USER` env → `USERNAME` env (Windows) → `"default"`. If `aida queue list` ever returns nothing where you expect items, check `echo $USER` and `echo $AIDA_USER` first — the queue is keyed off whichever the shell sees.

### Proactive requirements workflow

**Requirement-first development.** Before implementing any feature or fix, ensure a requirement exists:

1. Check if work has a SPEC-ID. If not: `aida add --title "..." --description "..." --status approved`
2. During coding, add trace comments: `// trace:FR-1-042 | ai:claude`
3. Before committing: use `/aida-commit` to ensure all changes are linked

If you work conversationally without explicit `/aida-req` calls, use `/aida-capture` at session end to review and capture any requirements that were discussed but not yet added.

### Plan archival

Every implementation plan must be saved to `docs/plans/YYYY-MM-DD-<slug>.md`. Include `## Related Requirements` (AIDA spec IDs) and `## Status` (In Progress → Completed). The `docs/plans/` directory is part of the standard project structure scaffolded by `aida init`.

## AIDA-developer workflow (only when working on AIDA itself)

```bash
# One-time: install shell helpers (aida-on / aida-off) into ~/.bashrc or ~/.zshrc
aida dev shell-init --install

# Per-shell: activate the in-repo build (pyenv-style)
aida-on                                # alias for: eval "$(aida dev activate)"
# now `aida` resolves to ./target/{release|debug}/aida (whichever is freshest)

aida dev status                        # confirms activation, shows binary mtime
aida dev serve                         # foreground supervisor for aida-server (8080) + vite (5173)
                                       # Ctrl+C stops both

aida-off                               # alias for: eval "$(aida dev deactivate)"
# back to the released aida on PATH
```

`aida dev activate` prepends `target/{release,debug}/` (whichever is more recently built) to PATH and prefixes the shell prompt with `(aida-debug)` or `(aida-release)` so you can see the active build at a glance. `aida dev deactivate` undoes both.

For releases, `scripts/release.sh {major|minor|patch|<explicit>}` bumps the workspace version, generates tag notes from `git log <prev>..HEAD`, commits, tags, and pushes (which triggers `.github/workflows/release.yml` to build and publish binary tarballs).

## Code traceability

### Inline trace comments

```rust
// trace:FR-1-042 | ai:claude
fn implement_feature() { ... }
```

Format: `// trace:<SPEC-ID> | ai:<tool>[:<confidence>]` where confidence is high (implied), `med` (40-80% AI), or `low` (<40% AI).

### Commit message format

```
[AI:tool] type(scope): description (REQ-ID)

Examples:
  [AI:claude] feat(auth): add login validation (FR-0042)
  [AI:claude:med] fix(api): handle null response (BUG-0023)
  chore(deps): update dependencies          (no REQ-ID needed)
  docs: update README                       (no REQ-ID needed)
```

Rules:
- `[AI:tool]` required when commit includes AI-assisted code (files with `trace:` comments)
- `type` required: feat, fix, docs, style, refactor, perf, test, build, ci, chore, revert
- `(scope)` optional: component or area affected
- `(REQ-ID)` required for feat/fix; optional for chore/docs
- Style/fmt-cleanup tasks (`style`/`fmt`/`refactor` types) must verify with `cargo fmt --all -- --check` (the `--check` flag exits non-zero on drift). Plain `cargo fmt --all` rewrites in place and silently masks dirty diffs — CI runs `--check` and will fail if you skip it locally. (TASK-66)

Set `AIDA_COMMIT_STRICT=true` to reject non-conforming commits.

## Claude Code skills

`aida init` scaffolds 28 skills under `.claude/skills/` and matching slash commands under `.claude/commands/`. Daily drivers: `/aida-req`, `/aida-implement`, `/aida-commit`, `/aida-capture`, `/aida-doc`, `/aida-search`, `/aida-plan`, `/aida-onboard`. Run `aida` (no args) for the full CLI, or `ls .claude/skills/` for the full skill catalog.

### MCP server

`aida mcp-serve` exposes requirements as MCP tools and resources for native Claude Code integration via `.mcp.json`. Tools: `list_requirements`, `show_requirement`, `add_requirement`, `update_requirement`, `search_requirements`, `add_comment`, `list_features`. Resources: `aida://project/summary`, `aida://requirements/tree`. The MCP server is the highest-leverage surface for the agent-context vision.

## Template architecture (CRITICAL for AIDA development)

AIDA has a dual-copy template system. Master templates live in `aida-core/templates/` and get embedded into the binary at compile time via `build.rs`. The project-local copy under `.claude/` mirrors them so this repo dogfoods its own scaffolding:

- `.claude/settings.json` is a single file-level symlink to `aida-core/templates/settings.json`.
- `.claude/skills/` and `.claude/commands/` are regular directories whose **files** are per-file symlinks into `aida-core/templates/{skills,commands}/` — managed by `make sync-templates`.
- `.claude/hooks/` is a regular directory; its files are also per-file symlinks, but only for the hooks this project actually wires up in `settings.json` (so the dir doesn't auto-grow with every new hook script that appears in the master templates). `make sync-templates` does NOT touch hooks today — link new ones by hand.

### When editing a skill, command, hook, or settings.json

1. Edit ONLY the master copy in `aida-core/templates/`
2. The symlinks ensure `.claude/` stays in sync
3. Run `make sync-templates` to verify symlinks
4. Changes embed into the next binary build

Hook commands in `settings.json` should use `$CLAUDE_PROJECT_DIR/...` paths so they resolve regardless of CWD when Claude Code invokes them.

## CLI reference (authoritative)

Always verify CLI arguments with `aida <command> --help`. Common parameters:

- `--type` (lowercase): `functional`, `non-functional`, `system`, `user`, `bug`, `epic`, `story`, `task`, `spike`, `sprint`, `folder`, `meta`, `doc`
- `--feature`: feature category name (NOT a type)
- `--status`: `draft`, `approved`, `in-progress`, `completed`, `rejected`
- `--priority`: `high`, `medium`, `low`

### Requirement types

Use `task` for chores, documentation, tooling, and work that doesn't fit a traditional requirement.

- **Requirements**: `functional`, `non-functional`, `system`, `user` (features, behaviors, constraints)
- **Agile**: `epic`, `story`, `task`, `bug`, `spike`, `sprint`
- **Organizational**: `folder` (hierarchy, stateless), `meta` (AI prompts, templates, stateless)
- **Living docs**: `doc` (EPIC-24 — narrative explanation linked to other specs via `aida doc add --about <ID>`)

### Meta requirements (AI prompt customization)

META requirements store AI prompts as editable requirements:

```bash
aida list --type meta              # List prompts
aida show META-002                 # View "Evaluate Requirement" prompt
aida edit META-002 --description "..."   # Customize AI behavior
```

Default META prompts seeded by `aida init`: META-002 (Evaluate), META-003 (Find Duplicates), META-004 (Suggest Relationships), META-005 (Improve Description), META-006 (Generate Children). The AI system checks DB prompts first, falls back to embedded defaults.

### Tree export/import

Export requirement hierarchies for sharing between projects:

```bash
aida export --format tree --id FOLDER-001 -o templates.json
aida import templates.json
aida import templates.json --parent FOLDER-002 --on-conflict skip
```

Conflict strategies: `skip`, `rename`, `replace`.
