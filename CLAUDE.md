# CLAUDE.md

Guidance for Claude Code working in the AIDA repository. Project background and high-level architecture live in `OVERVIEW.md`; this file focuses on conventions and reference material you need while writing code here.

## Project orientation

AIDA = AI Design Assistant. The defensible niche is the **agent-collaboration layer**: stable spec IDs, typed relationships, code-to-spec trace comments, and an MCP server that exposes the requirement graph to coding agents. Karpathy-style "structured markdown queryable by Claude" is the floor; AIDA adds the relationship graph + identifier stability + enforcement loop.

**Strategic positioning** (Trojan-horse, 2026-05-14): the visible product is intentionally simple — a TUI wrapping Claude Code sessions (EPIC-26). The *"so what? I could do this in 20 lines of bash"* reaction on first sight is the *intended* impression. The actual value — graph, IDs, traces, MCP, queue, lifecycle — surfaces through use, not the surface. When adding features or polish, the test is **"does this make the TUI's quiet depth stronger when someone digs in?"** Surface complexity in the TUI itself is anti-pattern. See `OVERVIEW.md` "Public face: the TUI is the product" section for the full framing.

For the full vision, architecture, and surface inventory see `OVERVIEW.md`. For the path-forward audit and current direction see `docs/plans/2026-05-02-git-canonical-storage.md`. For the **autonomy + escalation + inter-agent comms architecture** — the three-mode autonomy ladder, the implementer → advisor → human escalation cascade, the advisor's Type A/B/C calibration, and the file-based handshake substrate — see `docs/architecture/autonomy-and-escalation.md` (paired with `docs/autonomous-drain.md` for the practical user guide). For wider market landscape comparisons and refresh signals see `docs/competitive-analysis/`. For the *"should I use AIDA or X?"* question in any of its forms see `docs/positioning/` (one focused comparison per neighbor tool; lead with the two nearest competitors `vs-spec-kit.md` + `vs-kiro.md`, then `vs-ultrareview.md`, `vs-ultraplan.md`, `vs-claude-code-subagents.md`, `vs-claude-code-workflows.md`, `vs-agent-teams.md`, `vs-karpathy-md.md`, `vs-saas-pm.md`). The current competitive picture — the moat, the commoditized-vs-differentiated split, the capability roadmap, and the tripwires — is `docs/competitive-analysis/2026-05-31-round2-moat-gaps-moves.md`. For precise definitions of AIDA's machinery vocabulary — orchestrator, phase, drain, lease, role, scope, session, worktree, sentinel, batch, autonomy mode — see `aida-core/templates/docs/aida/discipline/machinery-glossary.md` (the scaffolded discipline-pack glossary; companions: `lifecycle-vocabulary.md` for spec-state verbs). For the TUI — keybindings, status overlay, autonomous drains, crash recovery — see `docs/tui/README.md` (`aida tui`, shipped default-on since STORY-137).

**Workspace** (6 crates): `aida-core` (engine), `aida-cli` (`aida` binary + MCP server), `aida-crate` (published `aida` crate metadata), `aida-server` (REST + gRPC, port 8080), `aida-generate-types` (Rust → TypeScript), `aida-tui` (the `aida tui` terminal shell, EPIC-26). React dashboard at `aida-web-react/` (port 5173 dev). Native desktop and WASM clients were extracted to a separate repo on 2026-05-02.

## Storage model (EPIC-1-001)

**Git-canonical by default.** The orphan `aida-store` branch is the writer of record (one YAML file per requirement under `objects/TYPE/000/SPEC-ID.yaml`). A SQLite cache at `.aida/cache.db` (gitignored, auto-rebuilt) is a rebuildable read projection used to make list/filter/search fast without scanning hundreds of YAML files. Writes go to git first, then the cache (write-through). Stale-detection compares the cache's recorded HEAD SHA against the orphan branch's actual HEAD; mismatch triggers rebuild on next read. The cache also self-heals on **schema drift** — a `PRAGMA table_info` check on open verifies the expected columns exist, and a missing column (e.g. a schema change a concurrent/older binary left half-applied) triggers drop+rebuild *before* the query instead of a hard "no such column" error (BUG-627). **Single-spec writes are targeted**: `aida edit`/`add`/`comment`/status-change resolve the one spec via the cache (any id form → its canonical YAML) and read/modify/write that one file — no full-store scan — so writes are sub-second even with active leases (was ~13-20s; BUG-634). Reads follow the same rule: the bare `aida status` and `aida list` never `backend.load()` the full store (STORY-707).

- Live worktree: `.aida-store/` (gitignored)
- Branch: `aida-store` on origin
- Cache: `.aida/cache.db` (gitignored)
- Manage cache: `aida cache status`, `aida cache rebuild`
- Compact store: `aida store compact` / `aida store gc` — substrate-tax relief for the orphan store: an aggressive, non-destructive `git gc --aggressive` repack. `--squash` (destructive history rewrite) is opt-in, gated behind `--yes` + an automatic backup; the bare command never rewrites history.
- Fresh clone: the first store-reading command **auto-attaches** the `.aida-store/` worktree from the `aida-store` branch and rebuilds the cache, so `aida list`/`findings`/`queue` work with no manual step (TASK-621). Writing new spec ids needs a node id — `aida init` (full bootstrap) or `aida node acquire`. If distributed mode is declared but the store can't be attached (offline, etc.), reads error with setup guidance rather than silently falling back to a legacy `requirements.yaml` (BUG-428). trace:TASK-621 trace:BUG-428

**Per-spec transition history lives in the orphan-branch GIT LOG.** **The source-of-truth for spec-state time series is the `aida-store` branch's git history** — every `aida edit` targeted-commits the one changed `objects/TYPE/000/SPEC-ID.yaml` (commit subject `update SPEC-ID`), so a status flip, priority change, tag edit, owner change, etc. is a commit whose before/after YAML diff *is* the structured row. `aida history` reads this: the default digest sorts by each YAML's `modified_at`, and `aida history --events [--id <spec-id|uuid>]` walks the git log and diffs each spec's YAML commit-over-commit to emit one typed event (`status: X → Y`, `priority: …`, `tags: +a -b`, …) per change. Reviewers building burn-down charts or status-flow analyses should run `aida history --events` (or walk the orphan-branch git log directly) rather than approximating from `modified_at` alone. `--id` accepts a spec_id, an agreed_id, or the raw UUID `aida show` prints (the UUID is resolved to its spec_id; BUG-588). The cache (`.aida/cache.db`) is a derived read-projection and does NOT expose history rows. (The `history:` array still exists on the `Requirement` model and is union-merged by `conflict.rs`, but the git-canonical `aida edit` path does NOT populate it — it is not the time-series substrate; the git log is.) The append-only `oplog.yaml` is the CRDT operation log used for conflict-free distributed sync (`conflict.rs` unions it on merge), not the surface `aida history` reads. trace:TASK-121 trace:BUG-588 | ai:claude

**`.gitignore` convention for `.aida/`:** deny-by-default. The `.gitignore` block scaffolded by `aida init` is `.aida/*` plus an explicit `!.aida/config.toml` allow-list. Anything new under `.aida/` is runtime per-clone state by convention; tracking a new project-config file requires adding a `!.aida/<name>` line. This avoids the recurring "new feature wrote a new runtime file, session-end refuses on the untracked path" papercut. trace:BUG-73 | ai:claude

PostgreSQL is opt-in via the `postgres` feature flag. Legacy standalone YAML/SQLite backends still exist for the deprecated `aida init --centralized` opt-in path; they print a deprecation warning at init time. **Don't** add new code paths that use them.

Architecture: `aida-core/src/dispenser.rs`, `node.rs`, `object_store.rs`, `db/git_backend.rs`, `db/cache.rs`, `db/cached_git_backend.rs`, `git_ops.rs`, `conflict.rs`.

**Every `AIDA_*` environment variable** — what it does, default, who sets it, scope — has one canonical reference: `docs/environment-variables.md`. When you add a new `AIDA_*` read, add a row there in the same change.

## Requirements management

This project uses AIDA for its own requirements tracking. **Do NOT maintain a separate `REQUIREMENTS.md` file** — the orphan-branch YAML files plus the cache are the source of truth. Use `aida list`, `aida search`, `aida show <ID>`, `aida add`, `aida edit`, `aida comment add` to work with them.

### Project initialization (for new projects, not this repo)

```bash
aida init                      # Default: distributed git-canonical (RECOMMENDED)
aida init --sibling            # Distributed using a sibling repo (multi-repo workspaces)
aida init --centralized        # Legacy SQLite mode (deprecated, prints warning)
aida init --no-skills          # Skip .claude/skills/ and .claude/commands/
aida init --no-hooks           # Skip .claude/hooks/ and git hooks
aida init --no-agent-config    # Skip the first-machine agent permission-posture prompt (~/.aida/agents.toml)
aida init --git-init           # Auto-run `git init` in a non-git folder (TTY offers it; flag opts in for scripts)
aida init --with-memories      # Also write the starter memory pack (opt-in)
aida init --with-memories --refresh   # Overlay updated pack files, keep your edits
aida init --with-memories --focus <subsystem>  # Scope the pack to a subsystem (untagged memories = universal, always loaded)
aida init --force              # Overwrite existing files
```

`aida init` creates: orphan branch `aida-store` + worktree at `.aida-store/`, `.aida/config.toml`, `.aida/cache.db`, META requirements seeded into the orphan store, `.mcp.json`, `CLAUDE.md`, `AGENTS.md` (Codex), `.claude/skills/` + `commands/` + `hooks/`, `docs/plans/`, `docs/aida/discipline/`.

**First-machine setup (global ~/.aida/).** On the first `aida init` on a machine, init also bootstraps machine-global agent defaults: the starter role set into `~/.aida/roles/` (TASK-638) and, at a TTY, a one-time prompt for the **agent permission posture** that writes `~/.aida/agents.toml` (TASK-698 — surfaces the STORY-495 `[agents] bypass` knob). The posture default is **native** (faithful launcher; Claude prompts); `bypass = true` is the explicit opt-in. Both steps are idempotent — an existing `~/.aida/agents.toml` / role file is never prompted-over or overwritten — and non-interactive init writes nothing (native default). Skip with `--no-roles` / `--no-agent-config`.

### First-user demo — `scripts/aida-demo.sh` (TASK-563)

To validate that `aida` is operational end-to-end on a fresh project — without polluting your real workspace — run the bundled demo script:

```bash
bash scripts/aida-demo.sh              # interactive walkthrough (Enter-to-continue between sections)
bash scripts/aida-demo.sh --auto-cleanup  # skip cleanup prompt (for CI / scripted runs)
```

The script creates a throwaway public GitHub repo (timestamped name like `aida-demo-20260525-...`), clones it locally, runs `aida init`, walks through filing a spec + implementing + committing with the `(SPEC-ID)` trailer convention + `aida pull` auto-bump, then prompts for cleanup (defaults to keep so you can poke around). Useful for: first-user evaluation, demo recordings, sanity-checking a fresh `aida` build initializes cleanly.

Prerequisites: `aida` on PATH (run `aida dev activate` first if using the dev build), `gh` CLI authenticated, `git` configured with `user.name` + `user.email`.

### Starter discipline pack (STORY-255)

`aida init` ships AIDA-using *discipline* — the habits and vocabulary that make an AIDA project run well — as scaffolding, so a new project inherits it instead of re-discovering the same friction. Three channels:

- **`docs/aida/discipline/`** (always) — the canonical guides: a `README.md` pointer-table plus per-topic files (`advisor-role.md`, `implementer-discipline.md`, `lifecycle-vocabulary.md`, `machinery-glossary.md`, `session-discipline.md`, `substrate-as-bouncer.md`, `brief-polling.md`, …; see the README table for the current set). Master templates: `aida-core/templates/docs/aida/discipline/`, embedded via `build.rs`, scaffolded by `ensure_discipline_pack_scaffold` (idempotent — `--force` to overwrite). Only the README `@`-imports into a downstream session's context; the per-topic files are read on demand (plain markdown links, no transitive load). `advisor-role.md` documents the **advisor** seat. `advisor` is the canonical role identifier everywhere — config, env vars (`AIDA_SESSION_ROLE`), queue routing, statusline, role files. `dialog` (the old internal token from TASK-279) is now a deprecated, silently-accepted alias for it, normalized to `advisor` at every role-name boundary so a not-yet-migrated machine's `dialog.toml`/config/shells keep working. trace:TASK-586 (supersedes TASK-279)
- **CLAUDE.md discipline section** (always) — `generate_claude_md` appends a "Discipline for AIDA-using sessions" section pointing at the pack.
- **Starter memory pack** (`--with-memories`, opt-in) — the generic discipline memories under `aida-core/templates/memories/` written to `~/.claude/projects/<slug>/memory/`. The pack is **marker-driven**: every memory file carrying `propagation: scaffolding-pack` in frontmatter ships, so the set grows just by tagging new generic memories. Scaffolded files get `originSessionId: aida-scaffold` + a `scaffoldChecksum` (FNV-1a of the body). `aida init --with-memories --refresh` overlays newer versions of files the user has *not* edited (body checksum still matches) and leaves edited or unmarked files alone. `MEMORY.md`'s `<!-- aida:scaffold-pack -->` block is regenerated; user content outside the markers is preserved.

When adding a new generic discipline memory, tag it `propagation: scaffolding-pack` and it joins the pack on the next build — no code change.

**Subsystem-scoped memories (STORY-362).** A memory file may also carry an optional `subsystem: <name>` frontmatter tag. `aida init --with-memories --focus <subsystem>` then loads only universal memories plus those whose `subsystem:` matches (case-insensitive). Backward-compatible: a memory with no `subsystem:` tag is **universal** and always loads, with or without `--focus`. Omitting `--focus` loads the full pack regardless of tags. (Forward-looking for SPIKE-10 subsystem-scoped advisors; the embedded pack is all-universal today.)

### Daily-use commands

```bash
aida do <SPEC> [--mode M] [--force]    # Universal dispatcher (STORY-776): routes on the advisor's groomed execution_mode (drain|drive|guided|operator|decide), printing the human contract first. Ungroomed at a TTY = propose+confirm micro-groom (reasoning line shown); headless = refused. --mode overrides one-shot: tighten free, loosen needs --force; `aida edit <SPEC> --mode M` is the durable advisor write
aida list                              # Cache-backed (sub-ms vs full-store load); default excludes archived
aida list --status draft               # Filter by status
aida list --archived                   # Only archived rows; --all = both (STORY-441)
aida list --fields id,status,title     # Select AND order the displayed columns — human table + agent/TOON output; unknown field errors with the valid set (STORY-734)
aida search "<query>"                  # Cache-backed FTS5 search (same archive filter as list); --fields selects columns too (STORY-734)
aida history                           # Recent activity, incl. freshly-Completed; archive hides long-tail (STORY-441)
aida archive <ID>                      # Mark a spec archived (hidden from default views, audit trail preserved)
aida archive --older-than 30d --dry-run   # Preview bulk sweep; drop --dry-run to apply
aida unarchive <ID>                    # Restore an archived spec
aida defer <ID> --until "<condition>"  # Park as primed/conditional work — hidden from default views, NOT filed away; records the revisit trigger (STORY-584)
aida list --deferred                   # Only deferred rows (the primed shelf) + each spec's revisit trigger; honors legacy deferred:* tags
aida undefer <ID>                      # Restore a deferred spec to the active view
aida show <ID>                         # Show requirement details + git linkage (commits/files/branch/PR — TASK-241)
aida show <ID> --no-git                # Skip the git-linkage section; --verbose expands it
aida graph <ID> --blocked-by           # Transitive cross-spec queries: blocked-by/blocks chains, --tree epic rollup, --impact reverse closure, --json (STORY-489)
aida add --title "..." --type story --status draft --tags "tag1,tag2"
aida edit <ID> --status completed
aida comment add <ID> "..."
aida db merge-gate                     # Assign agreed short IDs (FR-7-001 → FR-1)
aida db sync --pull --push             # Sync orphan branch with remote
aida fetch                             # Read-only two-leg refresh of remote refs (TASK-107)
aida fetch --code-only --quiet         # Background-safe code-leg-only refresh
aida db reconcile-status [--spec ID] [--since REF] [--dry-run]  # Replay Done→Completed bumps the pull missed (TASK-226)
aida cache status                      # Compare cache HEAD vs git HEAD
aida status                            # Sub-second cache snapshot (role/branch/queue depth/counts). Heavy PR/CI/liveness/hygiene diagnostics moved to `aida doctor`; `--full`/`--ci` keep the rich view (STORY-707)
aida status <spec>                     # Per-spec liveness: ● live / ⚠ STALE / flag-only + session/pid/started/elapsed (STORY-694). `aida why <spec>` flags a stalled in-flight lease instead of "being worked" (BUG-623)
aida ps                                # Global running-work table: every active session/agent with spec/role/pid/started/elapsed/live-vs-STALE, plus orphaned In-Progress specs (no live session backing the flag) (STORY-696)
aida awaiting [--notice] [--json]      # Unified coordination inbox — every channel where YOU are the gate in ONE place: mergeable PRs + unacked briefs + findings + reviewer verdicts + NeedsAttention escalations + UNREAD MAIL (folded in). Same "Awaiting you" report that leads `aida status`. `--notice` = compact one-line per-turn signal (the UserPromptSubmit hook injects it; cache/local-backed, NO network — PRs omitted from the line), silent when nothing awaits (STORY-741)
aida integrate [--json]                # Read-only integrator throughput view (no drain): focus-scoped queue + merge throughput off git log origin/main / .aida/events.jsonl (time-since-last-merge + main-idle indicator) + the aida ps running-work table. Cache-backed; honors AIDA_AGENT_OUTPUT (TASK-1034)
aida statusbar [--once|--plain|--restore-title]  # Ambient read-only OSC terminal-title meter: `aida · q:5 · live:2 · STALE:1 · you:3 (…)` refreshed on an interval (default 15s); cache/local-fast, no network, NOT a dispatch surface. `--plain` feeds tmux status-right; `--restore-title` = opt-in gnhf-style title save/restore (STORY-715)
aida usage --slowest [--limit N]       # Commands ranked by latency (p50/p95/max + count) — perf debugging (STORY-709)
aida usage --events [--cmd X] [--slower-than Nms]  # Raw recent command-event stream with durations (STORY-709)
aida memories check [--verbose] [--json]   # Drift between local memory pack and binary's embedded master; fix via init --with-memories --refresh (STORY-410)
aida plan verify <file> [--fix]        # Lint a plan: drifted refs, missing files/sections (--fix rewrites refs) (TASK-93)
aida skill lint [<skill>] [--json] [-q]  # Lint skills that reference a plan: run plan-verify on each docs/plans/*.md ref + raw-glyph check the skill body; non-zero on drift/missing (TASK-927)
aida lint <SPEC|--scope feature|task|story> [--json]  # Opt-in EARS-style quality lens: flag vague triggers / missing behavior / conflicts / low testability; suggests rewrites, never edits (TASK-0417)
aida plan helpers <spec> [--append <file>]  # Derive a 'Reusable helpers' section from the trace graph (TASK-94)
aida ultraplan <spec> [--stdout|--json]     # Assemble a rich /ultraplan prompt from spec context; copy to clipboard (TASK-113)
aida goal --batch|--epic|--spec|--pr|--queue-empty ...  # Derive a machine-checkable /goal condition; flags AND-compose; --copy/--invoke (TASK-242)
aida groom [--apply] [--max-approvals N] [--only-tag/--exclude-tag] [--risk] [--then-drain]  # Canonical headless advisor disposition pass: a cold-boot advisor proposes approve/reject/park/queue per open spec; propose-by-default, --apply executes. Policy under `[intake]`. Advisor-side analog of `burndown run`. (`aida assess` / `aida intake` are deprecated aliases, silently accepted, normalized to `groom`; `aida backlog groom` is a separate move-approved-onto-queue command, no collision.) trace:STORY-560 trace:STORY-708
aida changelog refresh|generate|preview     # Rewrite/print structured CHANGELOG.md (idempotent) (TASK-299)
aida queue gc [--for <role>] [--dry-run]    # Garbage-collect dead routed queue entries — prunes entries whose target spec is archived/Completed/Rejected; still-actionable (incl. Done — awaiting merge) survive. Sibling of `queue prune --orphaned` (which targets DELETED specs)
aida brief <agent> <SPEC> --note "..."      # Write/list/ack local pickup briefs under .aida/agent-briefs/ (list --for-agent, ack <path>)
aida agent new claude --role implementer|advisor --spec <ID>  # Supervised launcher w/ registry + role-context snapshot (--show-context)
aida worktree enter <EPIC|SPEC>        # One command → a ready worktree, cd'd in (bare; the aida() wrapper auto-evals the emitted `cd`). EPIC arg = scoping-only worktree (auto-focus). A single non-epic SPEC arg ALSO takes the implementer lease (spec → In Progress) so you can start working it by hand — NO agent launched. Idempotent re-enter. `aida worktree add <EPIC|SPEC>` = create + print path, no cd (STORY-716/STORY-742)
aida --asciinema <subcommand>          # Record a demo/training/audit cast under .aida/casts/ (falls back to ~/.aida/casts/)
```

- **Briefs** route work without scrollback (`.aida/agent-briefs/<agent>/`, local runtime state). MCP-speaking agents use the equivalent tools `list_briefs` / `read_brief` / `ack_brief`.
- **`aida agent new`**'s role-context snapshot is a *startup* snapshot only — keep polling briefs/MCP for work filed after launch.
- **MCP client setup + marketplace surfaces:** `docs/agents/aida-mcp-install-matrix.md` (Claude Code, Codex, Cursor, Windsurf, Continue, Cline, Copilot, Devin, Amp, …). Before publishing through a marketplace/registry, run `docs/security/marketplace-publication-checklist.md`.
- **`aida --asciinema`** no-ops gracefully without `asciinema` or a TTY.
- **`aida queue list`** appends a **Done — awaiting merge** section so freshly-shipped work stays visible until auto-bump; `--no-in-flight` / `--in-flight-only` narrow the view.

**Tag conventions.** Subcommand-identifying tags use the colon-namespaced `aida:<subcommand>[:<verb>]` form (`aida:queue:work`) so `aida list --tags 'aida:queue:*'` matches the surface; behavior / provenance / severity tags stay flat (`orchestrator`, `papercut`, `severity:cosmetic`, `batch:NAME`, `parent:EPIC-31`, `depends-on:phase-1`, …). `scripts/migrate-tag-namespace.sh` re-sweeps stray flat hyphen-forms. Full rules + anti-patterns: `docs/aida/discipline/tag-conventions.md`.

**Batch tags.** Items sharing `batch:NAME` (set via `aida edit <id> --tags batch:NAME`) compose: `aida queue list / work / progress --batch NAME` filter or drain that batch. `aida queue work --batch NAME --auto-complete` drains the whole batch — one implementer→CI→reviewer→merge→pull→build lifecycle per member (`=through-ci`/`through-merge` variants stop earlier). **EPIC-28 resilient drain**: a *shelvable* phase failure (CI red, RequestChanges, build fail) parks the spec `NeedsAttention` and the drain continues; dependents (`BlockedBy → <shelved>`) skip; the drain exits **`2`** when anything shelved/skipped so scripts triage. Cap with `--max-failures N` (default 5; `0` = first-failure-stops). Triage with `aida findings list`. Details: `docs/autonomous-drain.md`.

**Lifecycle short-circuit tags.** `lifecycle:no-ci-wait` / `no-review` / `no-build` each skip that one non-integrity phase during `--auto-complete` (`lifecycle:trivial` = all three). CI still runs remotely; merge + pull/auto-bump never skip. Use only for low-risk, small-blast-radius work.

**Calibration mode.** `[advisor] calibration_mode = "on"` (or `--calibrate` per-drain) makes every advisor punt emit two verdicts — cold-boot drives the drain, fork-from-live shadows — to mine substrate gaps; review with `aida findings calibration [--stats]`. Cost is real (both runs fire). Details: `docs/autonomous-drain.md`.

**Headless drain (`--no-human`).** `aida queue work --auto-complete --no-human` runs orchestrator phases headless (`claude -p`) for unattended drains; `--no-human=both` runs the implementer headless too. A headless implementer that hits a design-fork *punts* (parks `NeedsAttention`); the orchestrator routes the punt to a headless advisor tier (`/aida-advise`) that either resolves-and-resumes the implementer or escalates (`--escalate-blocks` default parks for triage; `--escalate-defaults` ships the defensible default). Trade-off: **interactive = better decisions, headless = better throughput.** Modes, escalation flags, and the SPIKE-7 evidence: `docs/autonomous-drain.md`.

### Queue identity (BUG-89)

The queue's `user_id` is the **shell's** user identity — not the node identity from `~/.aida/node.toml`, not the email in `[node]`, not the role's stored `user_id`. Every queue path (`add`, `list`, `next`, `done`, `remove`, `move`, the role-show queue head, the statusline depth) routes through `current_user_id()` in `aida-cli`, which resolves in order: `--user <id>` flag → `AIDA_USER` env → `USER` env → `USERNAME` env (Windows) → `"default"`. If `aida queue list` ever returns nothing where you expect items, check `echo $USER` and `echo $AIDA_USER` first — the queue is keyed off whichever the shell sees.

User matching is **case-insensitive** (TASK-951): comparisons route through `canonical_user_id` (trim + lowercase), so `Joe` matches `joe` and `Joe.Mooney@x` matches `joe.mooney@x` across machines. The fold is at **comparison only** — the stored queue key / assignee / lease owner keep their original casing (the BUG-89 invariant holds; no stored value is rewritten). Genuinely-different aliases for one person across hosts (`joe` vs `joe.mooney@gmail.com`) are *not* collapsed by case-folding — that needs an explicit person↔alias map (TASK-845, open).

### Proactive requirements workflow

**Requirement-first development.** Before implementing any feature or fix, ensure a requirement exists:

1. Check if work has a SPEC-ID. If not: `aida add --title "..." --description "..." --status approved`
2. During coding, add trace comments: `// trace:FR-1-042 | ai:claude`
3. Before committing: use `/aida-commit` to ensure all changes are linked

If you work conversationally without explicit `/aida-req` calls, use `/aida-capture` at session end to review and capture any requirements that were discussed but not yet added.

### Plan archival

Every implementation plan must be saved to `docs/plans/YYYY-MM-DD-<slug>.md`. Use `docs/plans/_TEMPLATE.md` (scaffolded by `aida init` from `aida-core/templates/plan-template.md`) as the starting structure — 11 sections cover Approach + diagram, Decisions, Files (in build-order), Critical Files, Reusable helpers, Risks + gotchas, Tests (named), Verification (executable), Followups, and Related. The header carries Date / Specs / Status / Complexity. trace:TASK-92

**Symbol refs over line refs.** When citing code from a plan, prefer symbol refs (`fn handle_pull_command`, `struct ImplementationInfo`) over line refs (`main.rs:19713`). Symbol refs survive edits; line refs drift fast and are often stale within hours of generation. Worked example: `docs/plans/2026-05-13-story-86-done-status.md`.

The plan tooling closes the loop end-to-end: `aida ultraplan <spec>` assembles a context-rich `/ultraplan` prompt (description + `## Acceptance` + graph context + the 11-section structure); `aida plan helpers <spec>` derives a "don't reimplement this" section from the trace graph; `aida plan verify <file>` lints for drifted refs / missing files+sections (exits non-zero → pre-commit-hook-able, `--fix` rewrites refs); `aida queue work <spec>` rides the plan's Critical-Files/Followups/Verification brief into the session (`/aida-pickup` leads with it); and reaching Done/Completed offers to file each `## Followups` bullet as a child TASK (idempotent via a `[aida:followups]` marker; `AIDA_AUTO_FOLLOWUPS=false` to opt out). trace:TASK-93 trace:TASK-94 trace:TASK-95 trace:TASK-96 trace:TASK-113

## AIDA-developer workflow (only when working on AIDA itself)

```bash
# One-time: install the `aida()` shell wrapper into ~/.bashrc or ~/.zshrc
aida dev shell-init --install

# Per-shell: activate the in-repo build (pyenv-style)
aida dev activate                      # the `aida()` wrapper auto-evals this — no `eval $(...)` needed
# now `aida` resolves to ./target/release/aida (release is the default pin)

aida dev status                        # confirms activation, shows binary mtime
aida dev serve                         # foreground supervisor for aida-server (8080) + vite (5173)
                                       # Ctrl+C stops both

aida dev deactivate                    # the wrapper auto-evals this too
# back to the released aida on PATH
```

`aida dev activate` prepends the chosen `target/{release,debug}/` to PATH. Bare `aida dev activate` pins the **release** profile by default (TASK-1158); `aida dev activate debug` pins debug, and `aida dev activate auto` (or `--auto`) is the explicit, sticky opt-in to the old freshest-wins selection — the newest binary whose embedded git SHA matches (or is an ancestor of) the current branch HEAD wins (TASK-221), falling back to most-recently-built with a `Warning:` when neither binary matches the current HEAD. `aida dev status` shows the active binary's SHA, current HEAD, and the match verdict (`exact match` / `ancestor of HEAD` / `DIVERGED from HEAD`). Prefixes the shell prompt with `(aida-debug)` or `(aida-release)` so the active build is visible at a glance. `aida dev deactivate` undoes both.

**Profile rule: release is the daily driver — and the default pin (bare `aida dev activate` now picks it); rebuild with `make build-fast`.** The dev binary is dogfooded on hot paths (statusline, hooks, MCP server, drains) where debug is several times slower, and `build-fast` makes incremental release rebuilds ~2 min, so debug's compile-speed edge no longer justifies it as a default. Debug is for *sessions*, not a lifestyle: attach-a-debugger work or a tight `cargo build -p` loop on one crate — flip in, flip back to release on the way out. Leaving a shell on the auto pin ("freshest of either profile wins") is how a stale debug binary ends up driving a drain (2026-07-18 incident). `cargo test` builds its own artifacts and never requires activating debug.

For releases, `scripts/release.sh {major|minor|patch|<explicit>}` bumps the workspace version, regenerates `CHANGELOG.md` via `aida changelog refresh --released-as v<new>` so the changelog commits *with* the version bump (TASK-299), generates tag notes from `git log <prev>..HEAD`, commits, tags, and pushes (which triggers `.github/workflows/release.yml` to build and publish binary tarballs).

**CI is split for alpha cycle time** (TASK-257). PR CI (`.github/workflows/ci.yml`) is **Linux-only** — ~3-5 min cycle. Windows + macOS are validated by `.github/workflows/cross-platform.yml`, which runs on a nightly cron (06:00 UTC) and on manual `workflow_dispatch`. Check the latest nightly results before relying on cross-platform behaviour: <https://github.com/joemooney/aida/actions/workflows/cross-platform.yml>. **Releases require cross-platform CI green within 24h of tagging** — `scripts/release.sh` calls `scripts/pre-release-check.sh` before the tag step, which reuses a `<24h` green run or dispatches a fresh `gh workflow run cross-platform.yml` and blocks on it. Opt out with `--skip-xplat-check` / `AIDA_SKIP_XPLAT_CHECK=1` (not recommended for a published release). Re-add Windows + macOS to PR CI once there are non-Linux users and the cross-platform matrix has been quiet for 2+ weeks.

**Cross-worktree cargo cache gotcha** (TASK-0396): cargo's `target/.fingerprint/` references absolute paths from the build that produced each artifact. If `aida session end` removes a worktree, subsequent `cargo build` from a sibling worktree can fail with errors pointing at the deleted worktree's paths. Recovery: `cargo clean -p <crate>` for the affected workspace members, or `cargo clean` for a full reset. See `docs/session-lifecycle.md` for the full recipe.

**Usage telemetry** (STORY-122): every `aida` invocation appends a single JSONL line at `~/.aida/usage.jsonl` with the command shape (e.g. `queue list`), `args_count`, `exit_code`, and `duration_ms`. Privacy floor: no argument values, no file paths, no requirement content. Opt out with `AIDA_TELEMETRY=0` or `[telemetry] enabled = false` in `.aida/config.toml`. Query with `aida usage` (top-20 in last 30d), `aida usage --unused 30d` (deprecation candidates), `aida usage --errors` (high error-rate commands), or `aida usage --json` for machine consumers. The monthly-cadence synthesis surface is `/aida-insights` (TASK-577) — wraps `aida usage` + `aida usage --auto-complete` + `aida findings calibration --stats` into the three top-line signals (most-used, drain success, calibration agreement) and the deprecation / UX-gap / orchestrator-fix / substrate-gap follow-ups they suggest. The log is local-only and never phoned home.

### Divergent-branch recovery

The convention behind the two-leg git-mirror verbs (`fetch` / `pull` / `push` / `rebase`) — what bundles, what's a deliberate non-mirror, and the `--code-only` / `--store-only` / `--dry-run` / `--json` rules a new verb must follow — is `docs/git-verb-surface.md` (TASK-109).

**Multi-hub drift prevention** (STORY-760): when a project has more than one hub (github `origin` + a personal gitlab mirror), keep both `main` and `aida-store` identical on every hub. `aida remote status` (and `aida doctor --category remote-drift`) *detect* divergence; `[store.sync] mirror_remotes = ["gitlab"]` *fans out* the store push best-effort (warns, never fails, on a mid-reconcile mirror leg). The full model — the three legs, the native-multi-pushurl caveat while the store is diverged, and the reconcile procedure — is `docs/multi-hub-sync.md`.

`aida pull` for the **code** leg uses `git pull --ff-only` by design (see `aida-cli/src/main.rs:20120` — refuses to surprise the working tree with auto-rebase). On divergence it prints `git pull --rebase origin main` as a hint. The **store** leg uses `--rebase` (line 19678) because store conflicts are rare and the worktree is AIDA-managed.

When the code leg refuses (or raw `git pull` hits "Need to specify how to reconcile"):

```bash
git fetch origin "$(git rev-parse --abbrev-ref HEAD)"
git log --oneline @{u}..HEAD     # what we have that origin doesn't
git log --oneline HEAD..@{u}     # what origin has that we don't
git log --name-only @{u}..HEAD --pretty= | sort -u   # files we touched
git log --name-only HEAD..@{u} --pretty= | sort -u   # files they touched
# No overlap → safe: git pull --rebase
# Overlap   → inspect; rebase + resolve, or git rebase --abort
```

Global config that makes raw `git pull` Just Work without per-incident decisions:

```bash
git config --global pull.rebase true
git config --global rebase.autoStash true
git config --global advice.diverging false
```

Tooling gaps tracked: TASK-97 (`aida pull --autorebase` opt-in safe-rebase), TASK-98 (`/aida-commit` pre-commit fetch + behind-check).

## Code traceability

### Inline trace comments

```rust
// trace:FR-1-042 | ai:claude
fn implement_feature() { ... }
```

Format: `// trace:<SPEC-ID> | ai:<tool>[:<confidence>]` where confidence is high (implied), `med` (40-80% AI), or `low` (<40% AI).

**SPEC-IDs stay in developer artifacts, never in user-facing output.** A SPEC-ID is a breadcrumb for someone who holds the requirement graph; to a first-user it's opaque noise. Keep `TASK-85` / `STORY-249` in commits, code comments, plan files, spec text, and telemetry — strip it from workflow hints, banners, error messages, CLI stdout/stderr, and `aida <cmd> --help` text. Watch the both-at-once trap: a `///` doc comment on a `clap` field is a code comment *and* `--help` output — keep the `trace:` marker as a plain `//` comment so it doesn't leak. Full rule + worked example: `docs/user-facing-text-conventions.md`. trace:TASK-268

### Commit message format

```
[AI:tool] type(scope): description (REQ-ID)

Examples:
  [AI:claude] feat(auth): add login validation (FR-0042)
  [AI:claude:med] fix(api): handle null response (BUG-0023)
  [AI:antigravity+claude] test(hooks): accept mixed authorship (TASK-509)
  chore(deps): update dependencies          (no REQ-ID needed)
  docs: update README                       (no REQ-ID needed)
```

Rules:
- `[AI:tool]` required when commit includes AI-assisted code (files with `trace:` comments); use `[AI:tool1+tool2]` for mixed-agent authorship, with optional confidence on the whole commit (`[AI:tool1+tool2:med]`)
- `type` required: feat, fix, docs, style, refactor, perf, test, build, ci, chore, revert
- `(scope)` optional: component or area affected
- `(REQ-ID)` required for feat/fix; optional for chore/docs
- Style/fmt-cleanup tasks (`style`/`fmt`/`refactor` types) must verify with `cargo fmt --all -- --check` (the `--check` flag exits non-zero on drift). Plain `cargo fmt --all` rewrites in place and silently masks dirty diffs — CI runs `--check` and will fail if you skip it locally. (TASK-66)

Set `AIDA_COMMIT_STRICT=true` to reject non-conforming commits.

## Claude Code skills

`aida init` scaffolds 48 skills under `.claude/skills/` and matching slash commands under `.claude/commands/`. Daily drivers: `/aida-req`, `/aida-implement`, `/aida-commit`, `/aida-capture`, `/aida-doc`, `/aida-search`, `/aida-plan`, `/aida-rebase`, `/aida-onboard`, `/aida-drain-queue`. The `/ultraplan` round-trip pair: `aida ultraplan <SPEC>` assembles the prompt, `/aida-import-plan <FILE>` lands the saved output back under `docs/plans/` (TASK-113/TASK-114). The advisor's narrative report: `/aida-digest [--since <window>] [--audience customer|team|self]` (STORY-252). The advisor's monthly telemetry-pattern review: `/aida-insights` (TASK-577). The keystone-implementation mode: `/aida-guided-implement <SPEC>` (or `aida queue work <SPEC> --guided`) drives a structured step-by-step decision dialog for an architecture/security/keystone spec — major forks decided up front as `AskUserQuestion`s, each answer recorded as a traceable ADR, then implement between answers, finishing with a PR for human review (no auto-merge); interactive-only, the supervised counterpart to the autonomous drain (STORY-735). Orchestrator-internal: `/aida-advise` is the headless advisor tier (STORY-306) — spawned by `--auto-complete --no-human=both` on a punt, not run by hand. Run `aida` (no args) for the full CLI, or `ls .claude/skills/` for the full skill catalog.

### MCP server

`aida mcp-serve` exposes requirements as MCP tools and resources for native Claude Code integration via `.mcp.json`. Core requirement tools: `list_requirements`, `show_requirement`, `add_requirement`, `update_requirement`, `search_requirements`, `add_comment`, `add_relationship`, `query_graph`, `list_features`, `history`. The server **also** exposes the inter-agent coordination surface — mailbox (`send_message`/`read_inbox`), briefs (`list_briefs`/`read_brief`/`ack_brief`), punts, directives, findings, queue, sessions, and roles (~60 tools total; `grep '"name":' aida-cli/src/mcp.rs` for the live list). Resources: `aida://project/summary`, `aida://requirements/tree`. The MCP server is the **typed/structural** surface for MCP-native clients. Note: AIDA's 2026-06-29 agent-surface benchmark found MCP costs ~2× the token-efficient CLI (`AIDA_AGENT_OUTPUT`/TOON) for identical tasks at equal-or-lower success, and on-demand schema loading doesn't close the gap — so the **CLI is the primary agent surface**; MCP is the typed option, not the default. (`bench/agent-surface/results/report.md` — the 72-cell 2026-06-29 run, now committed.)

The **7 core tool schemas mirror the current CLI surface** (STORY-82): the status/type enums are the full taxonomy; `list_requirements` filters on `tags`/`batch`/`parent`/`role`(`for`)/`in_flight` like `aida list`; `show_requirement` appends git linkage by default (`include_git`/`verbose`, matching `aida show`); `add_requirement` accepts `parent`/`feature`/`owner`; `update_requirement` edits `title`/`type`/`priority`/`tags`/`parent` (status transitions stay gated — approved/planned are advisor-only, completed is merge-driven); `search_requirements` narrows by `type`/`status`. When you add a new CLI filter or field, mirror it onto the matching MCP tool schema + handler so the two surfaces don't drift. trace:STORY-82

Long-running MCP servers self-respawn after handled requests when the on-disk `aida --version` reports a newer package version or a different build SHA for the same version. The current MCP response is flushed first; the next request runs on the new binary. If a client still appears stale, kill that agent's `aida mcp-serve` process and let the MCP client respawn it.

**This dev repo dogfoods its own MCP server.** A checked-in `.mcp.json` registers `aida mcp-serve` (resolved off PATH — run `aida dev activate` so it's the in-repo build) so Claude Code sessions working *in* this repo exercise the MCP tools, not just the CLI. MCP-vs-CLI parity gaps (schema drift, tool-response edge cases) therefore surface here first, in our own workflow, rather than only when downstream projects hit them. trace:TASK-253

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

For hook control-flow semantics, keep `docs/agents/session-communication.md` current. In particular, `continue: false` is terminal, a blocked `PreToolUse` call cannot produce a later `PostToolUse`, and headless approval gates should use `permissionDecision: "defer"` plus an external resume loop rather than a prompt that nobody can answer.

## CLI reference (authoritative)

Always verify CLI arguments with `aida <command> --help`. Common parameters:

- `--type` (lowercase, 19 total): `functional`, `non-functional`, `system`, `user`, `change-request`, `bug`, `epic`, `story`, `task`, `spike`, `sprint`, `folder`, `meta`, `principle`, `vision`, `constraint`, `decision`, `term`, `doc` (canonical source is the `RequirementType` enum in `aida-core/src/models.rs`; `aida schema` will become the reflection-derived source once STORY-538 / PR #691 merges)
- `--feature`: feature category name (NOT a type)
- `--status`: `draft`, `approved`, `planned`, `in-progress`, `done`, `completed`, `rejected`
  - The full state machine — Draft → Approved → Planned → In Progress → Done → Completed → Released, with the precise verb for each transition and the edge cases (cluster PRs, parallel pipelining, autonomous drains) — is documented in `docs/lifecycle.md` and the README's "Spec lifecycle" section. trace:TASK-273
  - **`done` vs `completed` (STORY-86)**: `done` means "work finished on a branch" (set by `aida queue done`). `completed` means "merged to the default branch." `aida pull` and `aida db sync --pull` auto-bump `done → completed` when a commit referencing the spec lands on main, so you typically don't set `--status completed` manually — let the merge promote it. **When the auto-bump misses** (BUG-96 made the YAML unreadable at pull time, or the spec flipped to Done after the referencing commit was already on local main), recover with `aida db reconcile-status` — a manual replay of the same scan over a wider window. Add `--spec SPEC-ID` for a targeted replay, `--since REF` to bound the range, `--dry-run` to preview without writing. trace:TASK-226
  - **archive ≠ status (STORY-441)**: `archived` is a view-level flag orthogonal to `status`. `aida list` / `aida history` / `aida search` hide archived rows by default; `--archived` shows only archived; `--all` shows both. A freshly-Completed spec is *not* archived — it stays visible in the default view until an explicit `aida archive <ID>`, a bulk `aida archive --older-than 30d --dry-run` (default csv: completed,rejected), or the opt-in auto-sweep on `aida pull` (gated on `[archive] auto_after_days = N` in `.aida/config.toml`, clamped to ≥7 days; opt-out with `AIDA_AUTO_ARCHIVE=0`). Archive ≠ deletion: the YAML, the audit trail, and the requirement graph all survive. trace:STORY-441
  - **deferred ≠ status, deferred ≠ archived (STORY-584)**: `deferred` is a *second* view-level flag orthogonal to both `status` and `archived` — the **three tiers are active (default) / deferred / archived**. Deferred is for primed/conditional work that returns on a trigger (e.g. "promote needs-triage IF the shelf grows", "decide on real demand when a slice verb ships"): hidden from the default open-work view but **not** filed away the way archive is. `aida list` / `aida history` / `aida search` hide deferred rows by default; `--deferred` shows only the deferred shelf (with each spec's **revisit trigger**); `--all` shows the union of all three tiers. Park with `aida defer <ID> --until "<condition>"` (the `--until` trigger is the one thing distinguishing deferred=prospective from archived=retrospective), restore with `aida undefer <ID>`. The default view also **honors the pre-existing `deferred:*` parking tags** (a spec carrying any `deferred:*` tag is treated as deferred for view purposes even without the flag), so the burndown/queue's parking convention and the list view now agree. Deferred ≠ deletion; the YAML/audit/graph survive. trace:STORY-584
  - **epic status is a read-only rollup (BUG-626)**: an EPIC's status is **derived from its children**, not set by hand — no children → Draft; ≥1 child In Progress (or partially shipped) → In Progress; all children Done/Completed → Done/Completed; a shelved (`NeedsAttention`) child with nothing else moving → NeedsAttention. `aida edit <epic> --status X` is **rejected** ("an epic's status is a read-only rollup of its children… change the children's statuses instead"; `--force` only for recovery/reject). So an epic auto-moves to In Progress when a child starts and to Completed when all children finish — `aida list` / `aida why` / `aida status` stay truthful with zero hand-maintenance. (Known edge, BUG-628 open: archived-completed children are under-counted in the rollup, so a fully-shipped epic whose children were archived can still read Draft.)
- `--priority`: `high`, `medium`, `low`

### Requirement types

The `RequirementType` enum (`aida-core/src/models.rs`) is the canonical source — 19 variants. Use `task` for chores, documentation, tooling, and work that doesn't fit a traditional requirement. (Once `aida schema` ships — STORY-538 / PR #691 — it becomes the reflection-derived list, so docs can point at it instead of re-hand-listing.)

- **Requirements**: `functional`, `non-functional`, `system`, `user` (features, behaviors, constraints)
- **Workflow**: `change-request` (`CR-N`) — a proposed change to an existing requirement/system (distinct from a `bug`, which records a defect)
- **Agile**: `epic`, `story`, `task`, `bug`, `spike`, `sprint`
- **Organizational**: `folder` (hierarchy, stateless), `meta` (AI prompts, templates, stateless)
- **ADR + knowledge-graph family** (FR-1-074, the docs-layer types that drive the `aida-docs` projection):
  - `principle` (`PRIN-N`) — constitution clause / non-negotiable principle governing how the project is built (stateless)
  - `vision` (`VIS-N`) — vision / target outcome: what we're building, for whom, by when (stateful)
  - `constraint` (`CON-N`) — external or technical constraint: regulation, dependency, deadline (stateful)
  - `decision` (`ADR-N`) — Architecture Decision Record: a recorded decision + its rationale (stateful: proposed / accepted / superseded / deprecated; in AIDA statuses `draft` = proposed and `approved` = accepted — the advisor records acceptance with `aida edit ADR-N --status approved`, and `--status accepted` is an input alias for it)
  - `term` (`TERM-N`) — glossary term / ubiquitous-language anchor (stateless)
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
