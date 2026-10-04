# CLAUDE.md

Guidance for Claude Code working in the AIDA repository. Project background and high-level architecture live in `OVERVIEW.md`; this file focuses on conventions and reference material you need while writing code here.

## Project orientation

AIDA = AI Design Assistant. The defensible niche is the **agent-collaboration layer**: stable spec IDs, typed relationships, code-to-spec trace comments, and an MCP server that exposes the requirement graph to coding agents. Karpathy-style "structured markdown queryable by Claude" is the floor; AIDA adds the relationship graph + identifier stability + enforcement loop.

**Strategic positioning** (VIS-2 / ADR-59 / TERM-5, 2026-09-26): AIDA captures the intent behind a system — requirements, decisions, rejected alternatives, and the links from code back to them — so coding agents work from it today and the project is far better placed to be rebuilt than from commits and a README alone (VIS-2; comparative, no whole-system regeneration promise). The humble first surface is `aida why <file:line>` plus the memory lane; depth is discovered through the store, not through a front-end. The TUI (EPIC-26) is **a view onto the control plane, not the product's face** (ADR-59). The control plane — queue, drain, orchestrator, leases, verdicts, merge gating — is the **corpus-integrity layer**: it keeps the store true while unreliable agent workers change the code, which is why it takes about half the engineering without being the product (TERM-5). When adding features or polish, the test is **"does this make the store easier to consult?"**. *Superseded history:* from 2026-05-14 this paragraph carried the Trojan-horse framing ("the visible product is intentionally a humble TUI; the platform is what you discover"); SPIKE-86 found the engineering record does not support it, and ADR-59 replaced it. See `OVERVIEW.md` "First surface" and "The four layers" for the full framing.

For the full vision, architecture, and surface inventory see `OVERVIEW.md`. For the path-forward audit and current direction see `docs/plans/2026-05-02-git-canonical-storage.md`. For the **autonomy + escalation + inter-agent comms architecture** — the three-mode autonomy ladder, the implementer → advisor → human escalation cascade, the advisor's Type A/B/C calibration, and the file-based handshake substrate — see `docs/architecture/autonomy-and-escalation.md` (paired with `docs/autonomous-drain.md` for the practical user guide). For wider market landscape comparisons and refresh signals see `docs/competitive-analysis/`. For the *"should I use AIDA or X?"* question in any of its forms see `docs/positioning/` (one focused comparison per neighbor tool; lead with the two nearest competitors `vs-spec-kit.md` + `vs-kiro.md`, then `vs-ultrareview.md`, `vs-ultraplan.md`, `vs-claude-code-subagents.md`, `vs-claude-code-workflows.md`, `vs-agent-teams.md`, `vs-karpathy-md.md`, `vs-saas-pm.md`). The current competitive picture — the moat, the commoditized-vs-differentiated split, the capability roadmap, and the tripwires — is `docs/competitive-analysis/2026-05-31-round2-moat-gaps-moves.md`. For precise definitions of AIDA's machinery vocabulary — orchestrator, phase, drain, lease, role, scope, session, worktree, sentinel, batch, autonomy mode — see `aida-core/templates/.aida/discipline/machinery-glossary.md` (the scaffolded discipline-pack glossary; companions: `lifecycle-vocabulary.md` for spec-state verbs); the distinction between a build-loop role/seat and a stakeholder persona gate is in `docs/architecture/roles-seats-and-personas.md`. For the TUI — keybindings, status overlay, autonomous drains, crash recovery, and the seam rule that keeps reads in `aida-core` while writes go through the CLI subprocess — see `docs/tui/README.md` (`aida tui`, shipped default-on since STORY-137).

**Workspace** (7 crates): `aida-core` (engine), `aida-cli` (thin `aida` binary stub), `aida-cli-lib` (the CLI implementation + MCP server; the binary calls its one pub symbol `main_entry` — STORY-772), `aida-crate` (published `aida` crate metadata), `aida-server` (REST + gRPC, port 8080), `aida-generate-types` (Rust → TypeScript), `aida-tui` (the `aida tui` terminal shell, EPIC-26). React dashboard at `aida-web-react/` (port 5173 dev). Native desktop and WASM clients were extracted to a separate repo on 2026-05-02.

## Storage model (EPIC-1-001)

**Git-canonical by default.** The orphan `aida-store` branch is the writer of record (one YAML file per requirement under `objects/TYPE/000/SPEC-ID.yaml`). A SQLite cache at `.aida/cache.db` (gitignored, auto-rebuilt) is a rebuildable read projection used to make list/filter/search fast without scanning hundreds of YAML files. Writes go to git first, then the cache (write-through). Stale-detection compares the cache's recorded HEAD SHA against the orphan branch's actual HEAD; mismatch triggers rebuild on next read. The cache also self-heals on **schema drift** — a `PRAGMA table_info` check on open verifies the expected columns exist, and a missing column (e.g. a schema change a concurrent/older binary left half-applied) triggers drop+rebuild *before* the query instead of a hard "no such column" error (BUG-627). **Single-spec writes are targeted**: `aida edit`/`add`/`comment`/status-change resolve the one spec via the cache (any id form → its canonical YAML) and read/modify/write that one file — no full-store scan — so writes are sub-second even with active leases (was ~13-20s; BUG-634). Reads follow the same rule: the bare `aida status` and `aida list` never `backend.load()` the full store (STORY-707).

- Live worktree: `.aida-store/` (gitignored)
- Branch: `aida-store` on origin
- Cache: `.aida/cache.db` (gitignored)
- Manage cache: `aida cache status`, `aida cache rebuild`, `aida cache verify [--fix] [--json]`
  - `cache status` compares HEAD SHAs — it catches a cache that is BEHIND the store, but **not** a row that disagrees while the two HEADs match. `cache verify` is that sweep: it cross-checks every cached `status` against the status the store projects (stored status, or the epic rollup for an EPIC) and exits non-zero on drift; `--fix` rebuilds and re-checks. Status drift is the dangerous class — it makes closed work look open (a Rejected epic rendering as Draft in `list`/`show`) and open work look closed. trace:BUG-771
- Compact store: `aida store compact` / `aida store gc` — substrate-tax relief for the orphan store: an aggressive, non-destructive `git gc --aggressive` repack. `--squash` (destructive history rewrite) is opt-in, gated behind `--yes` + an automatic backup; the bare command never rewrites history.
- Fresh clone: the first store-reading command **auto-attaches** the `.aida-store/` worktree from the `aida-store` branch and rebuilds the cache, so `aida list`/`findings`/`queue` work with no manual step (TASK-621). Writing new spec ids needs a node id — `aida init` (full bootstrap) or `aida node acquire`. If distributed mode is declared but the store can't be attached (offline, etc.), reads error with setup guidance rather than silently falling back to a legacy `requirements.yaml` (BUG-428). trace:TASK-621 trace:BUG-428

**Per-spec transition history lives in the orphan-branch GIT LOG.** **The source-of-truth for spec-state time series is the `aida-store` branch's git history** — every `aida edit` targeted-commits the one changed `objects/TYPE/000/SPEC-ID.yaml` (commit subject `update SPEC-ID`), so a status flip, priority change, tag edit, owner change, etc. is a commit whose before/after YAML diff *is* the structured row. `aida history` reads this: the default digest sorts by each YAML's `modified_at`, and `aida history events [--id <spec-id|uuid>]` walks the git log and diffs each spec's YAML commit-over-commit to emit one typed event (`status: X → Y`, `priority: …`, `tags: +a -b`, …) per change. Reviewers building burn-down charts or status-flow analyses should run `aida history events` (or walk the orphan-branch git log directly) rather than approximating from `modified_at` alone. `--id` accepts a spec_id, an agreed_id, or the raw UUID `aida show` prints (the UUID is resolved to its spec_id; BUG-588). The cache (`.aida/cache.db`) is a derived read-projection and does NOT expose history rows. (The `history:` array still exists on the `Requirement` model and is union-merged by `conflict.rs`, but the git-canonical `aida edit` path does NOT populate it — it is not the time-series substrate; the git log is.) The append-only `oplog.yaml` is the CRDT operation log used for conflict-free distributed sync (`conflict.rs` unions it on merge), not the surface `aida history` reads. trace:TASK-121 trace:BUG-588 | ai:claude

**`.gitignore` convention for `.aida/`:** deny-by-default. The `.gitignore` block scaffolded by `aida init` is `.aida/*` followed by an explicit allow-list, and that allow-list has **several** entries, not one. In this repo they are `!.aida/config.toml`, `!.aida/discipline/`, `!.aida/discipline/**` and `!.aida/aliases.toml` (see `.gitignore`, and `git ls-files '.aida/*'` for the authoritative list). **`.aida/discipline/` is TRACKED, not runtime state** — thirteen files are in git. The default for anything *new* under `.aida/` is per-clone runtime state; tracking a new project-config file requires adding a `!.aida/<name>` line. This avoids the recurring "new feature wrote a new runtime file, session-end refuses on the untracked path" papercut. trace:BUG-73 | ai:claude

**`.aida/config.toml` is tracked, therefore its branch-local copy is POLICY-ONLY configuration.** Because the file is committable, any pushed branch can ship its own copy — the same property that made BUG-1624 declare `.aida/session-env.sh` untrusted. The standing decision (BUG-1723, 2026-10-02) splits the config surface by consequence: a key read from the branch-local copy may tune *policy* (timeouts, thresholds, role lists, display preferences); it may **not** select an executable, a shell command, or a credential. Authority-bearing values come from the running binary, from outside the worktree (environment, `~/.aida/`), or from the trusted default-branch copy read via `trusted_config` (`git show origin/<default>:…`, TASK-969, fail-closed) — that is how code-executing keys like `[pr-rebase] smoke_check` are read. Every section/key read from the branch-local copy is enumerated and classified in `scripts/config-trust.toml`; the guard test `aida-cli-lib/src/tests/bug_1723_config_trust_tests.rs` fails on any unclassified read site or authority-shaped key, so adding a config key is a reviewed trust decision, not a drive-by. The one question to ask of any new key: *could this value make AIDA launch, open, or authenticate something?* If yes, it does not belong in the branch-local file. trace:BUG-1723 | ai:claude

**Personal dotfiles at the repo root are ignored by name, and a new one is a decision.** This repository is PUBLIC, and `.bashrc` / `.zshrc` / `.gitconfig` / `.netrc` / `.npmrc` and friends are exactly the filenames that accumulate machine-local and employer context — a proxy setting, an internal host, a work email. `.gitignore` lists them individually rather than using a root glob, so the rule says what it protects against and a genuinely new project dotfile still has to be decided on rather than silently swallowed. **Scaffolding that writes a new agent pack into the project root must decide which half it is writing — and the decision covers the *directory* a manifest points into, not only the manifest.** Portable project config is tracked (`.codex/config.toml`, three lines registering the MCP server, is the Codex twin of `.mcp.json`; `.codex/skills/` is a content pack like `.claude/skills/`). Machine-local wiring is ignored — `.codex/hooks.json`, which embeds absolute paths, **and `.codex/hooks/`, the handler directory it points at**. That second rule is the non-obvious one, because `.claude/hooks/` holds the *same ten script names* and is tracked. They are not the same kind of object: `.claude/hooks/` is tracked as ten **symlinks** into the master `aida-core/templates/hooks/` (so those bodies are in git exactly once) under a tracked, path-portable `.claude/settings.json`, whereas `scaffold_codex_hooks` **writes real copies** of the same embedded masters under a manifest that is deliberately ignored. Tracking them would duplicate already-tracked content and still not spare a fresh clone the scaffold pass — which writes handlers and wiring together anyway. Untracked-and-unignored is the one state that is neither, and it is the state `git add -A` publishes from. trace:BUG-1511 | ai:claude

**Trap — a blanket `.aida/` in `.git/info/exclude` silently defeats that allow-list.** Git cannot re-include a file whose *parent directory* is excluded, so one such line overrides every `!.aida/...` entry in `.gitignore` for files not already tracked. Already-tracked files keep working (ignore rules do not apply to tracked paths), which is what makes the symptom confusing: a hook or guard that consults *ignore* status rather than *tracked* status refuses paths that are demonstrably in git. `.git/info/exclude` is per-clone and untracked, so it differs between machines and is invisible to everyone else. Check it with `git check-ignore -v <path>` before concluding the repo's `.gitignore` is wrong. trace:BUG-1279 | ai:claude

PostgreSQL is opt-in via the `postgres` feature flag. Legacy standalone YAML/SQLite backends still exist for the deprecated `aida init --centralized` opt-in path; they print a deprecation warning at init time. **Don't** add new code paths that use them.

Architecture: `aida-core/src/dispenser.rs`, `node.rs`, `object_store.rs`, `db/git_backend.rs`, `db/cache.rs`, `db/cached_git_backend.rs`, `git_ops.rs`, `conflict.rs`.

**Every `AIDA_*` environment variable** — what it does, default, who sets it, scope — has one canonical reference: `docs/environment-variables.md`. When you add a new `AIDA_*` read, add a row there in the same change.

## Requirements management

This project uses AIDA for its own requirements tracking. **Do NOT maintain a separate `REQUIREMENTS.md` file** — the orphan-branch YAML files plus the cache are the source of truth. Use `aida list`, `aida search`, `aida show <ID>`, `aida add`, `aida edit`, `aida comment add` to work with them.

### Project initialization (for new projects, not this repo)

```bash
aida init                      # Default: distributed git-canonical (RECOMMENDED)
aida init <DIR> [--lang rust|python|node] [--github [--public] | --remote <url>]
                               # Bootstrap from NOTHING: create DIR, native-tool scaffold, git init +
                               # first commit, standard init inside, then remote + ordered pushes
                               # (code branch, THEN aida-store). Private repo by default (STORY-780)
aida init --sibling            # Distributed using a sibling repo (multi-repo workspaces)
aida init --centralized        # Legacy SQLite mode (deprecated, prints warning)
aida init --no-skills          # Skip .claude/skills/ and .claude/commands/
aida init --no-hooks           # Skip .claude/hooks/ and git hooks
aida init --no-agent-config    # Skip the first-machine agent permission-posture prompt (~/.aida/agents.toml)
aida init --git-init           # Auto-run `git init` in a non-git folder (TTY offers it; flag opts in for scripts)
aida init --with-memories      # Also write the starter memory pack (opt-in)
aida init --refresh            # Edit-preserving refresh of EVERY installed pack (memories,
                               # .claude/skills+commands, .codex/skills, .antigravity/skills,
                               # ~/.codex/prompts). Same as `aida scaffold refresh`; no --force
aida init --with-memories --focus <subsystem>  # Scope the pack to a subsystem (untagged memories = universal, always loaded)
aida init --force              # Overwrite existing files
```

`aida init` creates: orphan branch `aida-store` + worktree at `.aida-store/`, `.aida/config.toml`, `.aida/cache.db`, META requirements seeded into the orphan store, `.mcp.json`, `CLAUDE.md`, `AGENTS.md` (Codex), `.claude/skills/` + `commands/` + `hooks/`, `docs/plans/`, `.aida/discipline/`.

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

- **`.aida/discipline/`** (always) — the canonical guides: a `README.md` pointer-table plus per-topic files (`advisor-role.md`, `implementer-discipline.md`, `lifecycle-vocabulary.md`, `machinery-glossary.md`, `session-discipline.md`, `substrate-as-bouncer.md`, `brief-polling.md`, …; see the README table for the current set). Master templates: `aida-core/templates/.aida/discipline/`, embedded via `build.rs`, scaffolded by `ensure_discipline_pack_scaffold` (idempotent — `--force` to overwrite). Only the README `@`-imports into a downstream session's context; the per-topic files are read on demand (plain markdown links, no transitive load). `advisor-role.md` documents the **advisor** seat. `advisor` is the canonical role identifier everywhere — config, env vars (`AIDA_SESSION_ROLE`), queue routing, statusline, role files. `dialog` (the old internal token from TASK-279) is now a deprecated, silently-accepted alias for it, normalized to `advisor` at every role-name boundary so a not-yet-migrated machine's `dialog.toml`/config/shells keep working. trace:TASK-586 (supersedes TASK-279)
- **CLAUDE.md discipline section** (always) — `generate_claude_md` appends a "Discipline for AIDA-using sessions" section pointing at the pack.
- **Starter memory pack** (`--with-memories`, opt-in) — the generic discipline memories under `aida-core/templates/memories/` written to `~/.claude/projects/<slug>/memory/`. The pack is **marker-driven**: every memory file carrying `propagation: scaffolding-pack` in frontmatter ships, so the set grows just by tagging new generic memories. Scaffolded files get `originSessionId: aida-scaffold` + a `scaffoldChecksum` (FNV-1a of the body). `aida init --with-memories --refresh` overlays newer versions of files the user has *not* edited (body checksum still matches) and leaves edited or unmarked files alone. `MEMORY.md`'s `<!-- aida:scaffold-pack -->` block is regenerated; user content outside the markers is preserved.

When adding a new generic discipline memory, tag it `propagation: scaffolding-pack` and it joins the pack on the next build — no code change.

**Cross-vendor pack refresh (TASK-1170, BUG-1464).** The memory pack's edit-preserving `--refresh` contract now covers **every** agent pack: `aida scaffold refresh` (equivalently `aida init --refresh`) brings `.claude/skills/`, `.claude/commands/`, `.codex/skills/`, `.antigravity/skills/`, the machine-global `~/.codex/prompts/`, and installed starter roles level with the binary's embedded content. A file whose body still hashes to the `AIDA Generated: … | checksum:…` marker it was written with is overlaid; a file you edited, a file with no marker, and a **symlinked destination** (the dev-repo layout — BUG-718) are left exactly as they are; packs you never installed are not created. Starter roles predate those markers, so refresh only appends a shipped `system_prompt` when that field is absent, preserving every existing byte and never replacing a user prompt. This is the delivery path for a template or starter-guidance fix — fixing a command/skill master reaches Claude, Codex and Antigravity through one mechanism, without `--force`. Core: `aida-core/src/scaffolding/refresh.rs`; driver: `aida-cli-lib/src/scaffold_refresh.rs`.

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
aida graph tree <ID>                    # Epic rollup; use blocked-by/blocks/impact modes for transitive queries (STORY-489)
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
aida status <spec>                     # Per-spec liveness: ● live / ⚠ STALE / flag-only + session/pid/started/elapsed (STORY-694). `aida why <spec>` flags a stalled in-flight lease instead of "being worked" (BUG-623). `aida why <file:line>` = code→decision: nearest `trace:` comment first, then git-blame `(SPEC-ID)`-trailer fallback with commit provenance (`resolved_via: trace|blame` in JSON) (STORY-785)
aida ps                                # Global running-work table: every active session/agent with spec/role/pid/started/elapsed/live-vs-STALE, plus orphaned In-Progress specs (no live session backing the flag) (STORY-696)
aida tail [<session|SPEC|drain>] [--list] [--json] [-n N] [--since D] [--no-follow] [--no-timestamp]  # Stream a running session's log BY ID — resolves a session id from `aida ps`, a spec id, a drain id, or the keyword `drain` to the one file under `.aida/` that work streams into (`.aida/burndown/<drain-id>.jsonl` or `.aida/headless-logs/<branch>-<uuid>.jsonl`) and follows it. Bare = newest log; `--list` = every log + which session owns it; `--json` = raw stream-json. Each rendered line is prefixed with its OWN event time in local tz (`[23:14:07] …`) so the gaps between lines ARE the pacing signal — an event with no timestamp falls back to arrival wall-clock, never a wrong-but-confident stamp; `--no-timestamp` drops the prefix for clean copy-paste. A session with no log (interactive) reports cleanly, exit 0 (TASK-1167, TASK-1173)
aida awaiting [--notice] [--json]      # Unified coordination inbox — every channel where YOU are the gate in ONE place: mergeable PRs + unacked briefs + findings + reviewer verdicts + NeedsAttention escalations + UNREAD MAIL (folded in). Same "Awaiting you" report that leads `aida status`. `--notice` = compact one-line per-turn signal (the UserPromptSubmit hook injects it; cache/local-backed, NO network — PRs omitted from the line), silent when nothing awaits (STORY-741)
aida integrate [--json]                # Read-only integrator throughput view (no drain): focus-scoped queue + merge throughput off git log origin/main / .aida/events.jsonl (time-since-last-merge + main-idle indicator) + the aida ps running-work table. Cache-backed; honors AIDA_AGENT_OUTPUT (TASK-1034)
aida statusbar [--once|--plain|--restore-title]  # Ambient read-only OSC terminal-title meter: `aida · q:5 · live:2 · STALE:1 · you:3 (…)` refreshed on an interval (default 15s); cache/local-fast, no network, NOT a dispatch surface. `--plain` feeds tmux status-right; `--restore-title` = opt-in gnhf-style title save/restore (STORY-715)
aida usage --limit N slowest         # Commands ranked by latency (p50/p95/max + count) — perf debugging (STORY-709)
aida usage --cmd X --slower-than Nms events  # Raw recent command-event stream with durations (STORY-709)
aida usage --cmd X --slower-than Nms timeline  # Compact one-line-per-invocation feed: local ts, duration, cmd, pass/fail mark — the events stream, denser (TASK-1481)
aida memories check [--verbose] [--json]   # Drift between local memory pack and binary's embedded master; fix via init --with-memories --refresh (STORY-410)
aida plan verify <file> [--fix]        # Lint a plan: drifted refs, missing files/sections (--fix rewrites refs) (TASK-93)
aida skill lint [<skill>] [--json] [-q]  # Lint skills that reference a plan: run plan-verify on each docs/plans/*.md ref + raw-glyph check the skill body; non-zero on drift/missing (TASK-927)
aida lint <SPEC|--scope feature|task|story> [--json]  # Opt-in EARS-style quality lens: flag vague triggers / missing behavior / conflicts / low testability; suggests rewrites, never edits (TASK-0417)
aida plan helpers <spec> [--append <file>]  # Derive a 'Reusable helpers' section from the trace graph (TASK-94)
aida ultraplan <spec> [--stdout|--json]     # Assemble a rich planner prompt from spec context; copy to clipboard (TASK-113)
aida goal --batch|--epic|--spec|--pr|--queue-empty ...  # Derive a machine-checkable /goal condition; flags AND-compose; --copy/--invoke (TASK-242)
aida groom [--apply] [--max-approvals N] [--only-tag/--exclude-tag] [--risk] [--then-drain]  # Canonical headless advisor disposition pass: a cold-boot advisor proposes approve/reject/park/queue per open spec; propose-by-default, --apply executes. Policy under `[intake]`. Advisor-side analog of `burndown run`. (`aida assess` / `aida intake` are deprecated aliases, silently accepted, normalized to `groom`; `aida backlog groom` is a separate move-approved-onto-queue command, no collision.) trace:STORY-560 trace:STORY-708
aida changelog refresh|generate|preview     # Rewrite/print structured CHANGELOG.md (idempotent) (TASK-299)
aida queue gc [--for <role>] [--dry-run]    # Garbage-collect dead routed queue entries — prunes entries whose target spec is archived/Completed/Rejected; still-actionable (incl. Done — awaiting merge) survive. Sibling of `queue prune --orphaned` (which targets DELETED specs)
aida brief <agent> <SPEC> --note "..."      # Write/list/ack local pickup briefs under .aida/agent-briefs/ (list --for-agent, ack <path>)
aida agent new claude --role implementer|advisor --spec <ID>  # Supervised launcher w/ registry + role-context snapshot (--show-context)
aida worktree enter <EPIC|SPEC>        # One command → a ready worktree, cd'd in (bare; the aida() wrapper auto-evals the emitted `cd`). EPIC arg = scoping-only worktree (auto-focus). A single non-epic SPEC arg ALSO takes the implementer lease (spec → In Progress) so you can start working it by hand — NO agent launched. Idempotent re-enter. `aida worktree add <EPIC|SPEC>` = create + print path, no cd (STORY-716/STORY-742)
aida session reap [--dry-run] [-y] [--json]  # Supervisor pass that tears down FINISHED sessions whole — worktree removed + lease released + branch deleted — for every lease where spec is Done/Completed AND branch is merged AND the process has EXITED. Substrate-state only (spec status + git ancestry/merged PR + process liveness); NEVER scrapes a terminal and NEVER force-closes a live agent (a running session fails the predicate and is left untouched, reaped on a later pass once it exits). Reuses the `worktree gc` safety checks (dirty / locked / unique-unmerged-commit worktrees are never removed) and runs automatically after an AIDA-managed merge lands (TASK-1177). The same pass NOTIFIES a live-but-finished interactive session (spec merged + branch merged, but process still alive so never reaped): a once-per-session mailbox FYI (surface-only, no action) — "spec merged, safe to exit; the worktree reaps once you exit" — surfaced on the session's next turn via `aida awaiting`, deduped by an `.aida/session-notices/` sentinel; detect-and-notify, never scrape a terminal, never force-close (FR-284)
aida worktree reclaim [--apply] [--min-age-mins N] [--target-pct N] [--include-live] [--include-open-prs] [--json]  # Reclaim disk by deleting stale `target/` build caches inside this repo's worktrees. DRY RUN BY DEFAULT: reports reclaimable bytes and every worktree it held back, and deletes only with --apply. Where `worktree gc` removes worktrees that are FINISHED, `reclaim` removes the regenerable cache inside worktrees that are KEPT — the space `session reap` cannot reach and `cargo clean` targets wrongly (inside a worktree it clears the SHARED CARGO_TARGET_DIR, which points at the main checkout, so it deletes the live cache and leaves every stale one). Six rails: candidates come only from `git worktree list --porcelain`; the main checkout's `target/` is refused at the delete site, not merely filtered; `.aida-store` is never a candidate; a worktree held by a LIVE session lease is skipped (liveness via the shared staleness predicate, not a bare kill -0); a branch with an OPEN PR is skipped — and so is EVERY branch when open-PR state cannot be read, which fails safe; a `target/` touched within --min-age-mins is skipped as a possible in-flight build (TASK-1562)
aida --asciinema <subcommand>          # Record a demo/training/audit cast under .aida/casts/ (falls back to ~/.aida/casts/)
```

- **Briefs** route work without scrollback (`.aida/agent-briefs/<agent>/`, local runtime state). MCP-speaking agents use the equivalent tools `list_briefs` / `read_brief` / `ack_brief`.
- **`aida agent new`**'s role-context snapshot is a *startup* snapshot only — keep polling briefs/MCP for work filed after launch.
- **MCP client setup + marketplace surfaces:** `docs/agents/aida-mcp-install-matrix.md` (Claude Code, Codex, Cursor, Windsurf, Continue, Cline, Copilot, Devin, Amp, …). Before publishing through a marketplace/registry, run `docs/security/marketplace-publication-checklist.md`.
- **`aida --asciinema`** no-ops gracefully without `asciinema` or a TTY.
- **`aida queue list`** appends a **Done — awaiting merge** section so freshly-shipped work stays visible until auto-bump; `--no-in-flight` / `--in-flight-only` narrow the view.

**Tag conventions.** Subcommand-identifying tags use the colon-namespaced `aida:<subcommand>[:<verb>]` form (`aida:queue:work`) so `aida list --tags 'aida:queue:*'` matches the surface; behavior / provenance / severity tags stay flat (`orchestrator`, `papercut`, `severity:cosmetic`, `batch:NAME`, `parent:EPIC-31`, `depends-on:phase-1`, …). `scripts/migrate-tag-namespace.sh` re-sweeps stray flat hyphen-forms. Full rules + anti-patterns: `.aida/discipline/tag-conventions.md`.

**Batch tags.** Items sharing `batch:NAME` (set via `aida edit <id> --add-tag batch:NAME` — never `--tags`, which replaces the whole set) compose: `aida queue list / work / progress --batch NAME` filter or drain that batch. `aida queue work --batch NAME --auto-complete` drains the whole batch — one implementer→CI→reviewer→merge→pull→build lifecycle per member (`=through-ci`/`through-merge` variants stop earlier). To ship a batch as **one branch / one PR** (fix-test-review-CI a set of issues as a unit) add `--single-branch`; `--sequential` keeps member order on that branch (TASK-1003; SPIKE-81 found these undiscoverable). **EPIC-28 resilient drain**: a *shelvable* phase failure (CI red, RequestChanges, build fail) parks the spec `NeedsAttention` and the drain continues; dependents (`BlockedBy → <shelved>`) skip; the drain exits **`2`** when anything shelved/skipped so scripts triage. Cap with `--max-failures N` (default 5; `0` = first-failure-stops). Triage with `aida findings list`. Details: `docs/autonomous-drain.md`.

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

The plan tooling closes the loop end-to-end: `aida ultraplan <spec>` assembles a context-rich planner prompt (description + `## Acceptance` + graph context + the 11-section structure) for `/aida-plan`, a Plan agent, a multi-agent workflow, or human planning review; `aida plan helpers <spec>` derives a "don't reimplement this" section from the trace graph; `aida plan verify <file>` lints for drifted refs / missing files+sections (exits non-zero → pre-commit-hook-able, `--fix` rewrites refs); `aida queue work <spec>` rides the plan's Critical-Files/Followups/Verification brief into the session (`/aida-pickup` leads with it); and reaching Done/Completed offers to file each `## Followups` bullet as a child TASK (idempotent via a `[aida:followups]` marker; `AIDA_AUTO_FOLLOWUPS=false` to opt out). trace:TASK-93 trace:TASK-94 trace:TASK-95 trace:TASK-96 trace:TASK-113 trace:BUG-1177

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

**Profile rule: release is the daily driver — and the default pin (bare `aida dev activate` now picks it); rebuild with `make build-fast`.** The dev binary is dogfooded on hot paths (statusline, hooks, MCP server, drains) where debug is several times slower, and `build-fast` makes incremental release rebuilds ~2 min, so debug's compile-speed edge no longer justifies it as a default. Debug is for *sessions*, not a lifestyle: attach-a-debugger work or a tight `cargo build -p` loop on one crate — flip in, flip back to release on the way out. Leaving a shell on the auto pin ("freshest of either profile wins") is how a stale debug binary ends up driving a drain (2026-07-18 incident). `cargo test` builds its own artifacts and never requires activating debug. A live wave pins its launcher binary: `make build-fast` refuses to replace it, while `make build-fast AFTER_WAVE=1` parks until `QueueDrained`. Activation only updates the calling shell's PATH, so `aida dev activate` remains available while a wave is live. trace:TASK-1285

For releases, `scripts/release.sh {major|minor|patch|<explicit>}` bumps the workspace version, regenerates `CHANGELOG.md` via `aida changelog refresh --released-as v<new>` so the changelog commits *with* the version bump (TASK-299), generates tag notes from `git log <prev>..HEAD`, commits, tags, and pushes (which triggers `.github/workflows/release.yml` to build and publish binary tarballs).

**CI is split for alpha cycle time** (TASK-257). PR CI (`.github/workflows/ci.yml`) is **Linux-only** — ~9-10 min median cycle (measured 2026-09-18 over 23 PR runs: p25 9.3, p75 9.9; SPIKE-81 corrected the earlier "~3-5 min" claim). Windows + macOS are validated by `.github/workflows/cross-platform.yml`, which runs on a nightly cron (06:00 UTC) and on manual `workflow_dispatch`. Three PR-time guards now catch most Linux-only assumptions before the nightly: a report-only Linux-hosted `cargo check --workspace --tests` attempt for Windows/macOS targets in `ci.yml`, a required Rust-test portability ratchet in `scripts/check-portability.sh`, and a path-filtered informational Windows/macOS matrix for fragile worktree/session/git-backend/workflow changes. The Linux-hosted target check is report-only until the workspace's foreign C dependencies (`ring`, SQLite) can be checked for MSVC/Darwin from Linux without installing full foreign SDKs. Check the latest nightly results before relying on cross-platform behaviour: <https://github.com/joemooney/aida/actions/workflows/cross-platform.yml>. **Releases require cross-platform CI green within 24h of tagging** — `scripts/release.sh` calls `scripts/pre-release-check.sh` before the tag step, which reuses a `<24h` green run or dispatches a fresh `gh workflow run cross-platform.yml` and blocks on it. Opt out with `--skip-xplat-check` / `AIDA_SKIP_XPLAT_CHECK=1` (not recommended for a published release). Re-add Windows + macOS to PR CI once there are non-Linux users and the cross-platform matrix has been quiet for 2+ weeks.

**Cross-worktree cargo cache gotcha** (TASK-0396): cargo's `target/.fingerprint/` references absolute paths from the build that produced each artifact. If `aida session end` removes a worktree, subsequent `cargo build` from a sibling worktree can fail with errors pointing at the deleted worktree's paths. Recovery: `cargo clean -p <crate>` for the affected workspace members, or `cargo clean` for a full reset. See `docs/session-lifecycle.md` for the full recipe.

**Usage telemetry** (STORY-122): every `aida` invocation appends a single JSONL line at `~/.aida/usage.jsonl` with the command shape (e.g. `queue list`), `args_count`, `exit_code`, and `duration_ms`. Privacy floor: no argument values, no file paths, no requirement content. Opt out with `AIDA_TELEMETRY=0` or `[telemetry] enabled = false` in `.aida/config.toml`. Query with `aida usage` (top-20 in last 30d), `aida usage unused 30d` (deprecation candidates), `aida usage errors` (high error-rate commands), or `aida usage --json` for machine consumers. The monthly-cadence synthesis surface is `/aida-insights` (TASK-577) — wraps `aida usage` + `aida usage drains` + `aida findings calibration --stats` into the three top-line signals (most-used, drain success, calibration agreement) and the deprecation / UX-gap / orchestrator-fix / substrate-gap follow-ups they suggest. The log is local-only and never phoned home.

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

`aida init` scaffolds 48 skills under `.claude/skills/` and matching slash commands under `.claude/commands/`. Daily drivers: `/aida-req`, `/aida-implement`, `/aida-commit`, `/aida-capture`, `/aida-doc`, `/aida-search`, `/aida-plan`, `/aida-rebase`, `/aida-onboard`, `/aida-drain-queue`. The plan-prompt round trip: `aida ultraplan <SPEC>` assembles a rich prompt for `/aida-plan` or any other planner, and `/aida-import-plan <FILE>` lands a saved plan back under `docs/plans/` (TASK-113/TASK-114). The advisor's narrative report: `/aida-digest [--since <window>] [--audience customer|team|self]` (STORY-252). The advisor's monthly telemetry-pattern review: `/aida-insights` (TASK-577). The keystone-implementation mode: `/aida-guided-implement <SPEC>` (or `aida queue work <SPEC> --guided`) drives a structured step-by-step decision dialog for an architecture/security/keystone spec — major forks decided up front as `AskUserQuestion`s, each answer recorded as a traceable ADR, then implement between answers, finishing with a PR for human review (no auto-merge); interactive-only, the supervised counterpart to the autonomous drain (STORY-735). Orchestrator-internal: `/aida-advise` is the headless advisor tier (STORY-306) — spawned by `--auto-complete --no-human=both` on a punt, not run by hand. The operator-proxy orchestrator: `/aida-orchestrate` drives the whole open list to merged. It triages drafts as a recorded proxy, gates architecture work on an independent sketch signoff, fans out implementer subagents in worktrees, and has a fresh reviewer check every branch. Approved branches land through batched integration PRs that are merged on the exact commit and verified. Items that need a human are reported, never faked (STORY-1474). Codex and Antigravity get a vendor-neutral version in `.codex/skills/aida-orchestrate/` and `.antigravity/skills/aida-orchestrate/`, which runs implementers and reviewers as separate `aida queue work` / `aida agent new` sessions instead of subagents (STORY-1475). Run `aida` (no args) for the full CLI, or `ls .claude/skills/` for the full skill catalog.

### MCP server

`aida mcp-serve` exposes requirements as MCP tools and resources for native Claude Code integration via `.mcp.json`. Core requirement tools: `list_requirements`, `show_requirement`, `add_requirement`, `update_requirement`, `search_requirements`, `add_comment`, `add_relationship`, `query_graph`, `list_features`, `history`. The server **also** exposes the inter-agent coordination surface — mailbox (`send_message`/`read_inbox`), briefs (`list_briefs`/`read_brief`/`ack_brief`), punts, directives, findings, queue, sessions, and roles (~60 tools total; `grep '"name":' aida-cli/src/mcp.rs` for the live list). Resources: `aida://project/summary`, `aida://requirements/tree`. The MCP server is the **typed/structural** surface for MCP-native clients. Note: AIDA's 2026-06-29 agent-surface benchmark found MCP costs ~2× the token-efficient CLI (`AIDA_AGENT_OUTPUT`/TOON) for identical tasks at equal-or-lower success, and on-demand schema loading doesn't close the gap — so the **CLI is the primary agent surface**; MCP is the typed option, not the default. (`bench/agent-surface/results/report.md` — the 72-cell 2026-06-29 run, now committed.)

The **7 core tool schemas mirror the current CLI surface** (STORY-82): the status/type enums are the full taxonomy; `list_requirements` filters on `tags`/`batch`/`parent`/`role`(`for`)/`in_flight` like `aida list`; `show_requirement` appends git linkage by default (`include_git`/`verbose`, matching `aida show`); `add_requirement` accepts `parent`/`feature`/`owner`; `update_requirement` edits `title`/`type`/`priority`/`tags`/`parent` (status transitions stay gated — approved/planned are advisor-only, completed is merge-driven); `search_requirements` narrows by `type`/`status`. When you add a new CLI filter or field, mirror it onto the matching MCP tool schema + handler so the two surfaces don't drift. trace:STORY-82

Long-running MCP servers self-respawn after handled requests when the on-disk `aida --version` reports a newer package version or a different build SHA for the same version. The current MCP response is flushed first; the next request runs on the new binary. If a client still appears stale, kill that agent's `aida mcp-serve` process and let the MCP client respawn it.

**This dev repo dogfoods its own MCP server.** A checked-in `.mcp.json` registers `aida mcp-serve` (resolved off PATH — run `aida dev activate` so it's the in-repo build) so Claude Code sessions working *in* this repo exercise the MCP tools, not just the CLI. MCP-vs-CLI parity gaps (schema drift, tool-response edge cases) therefore surface here first, in our own workflow, rather than only when downstream projects hit them. trace:TASK-253

## Template architecture (CRITICAL for AIDA development)

AIDA has a dual-copy template system. Master templates live in `aida-core/templates/` and get embedded into the binary at compile time via `build.rs`. The project-local copy under `.claude/` mirrors them so this repo dogfoods its own scaffolding:

- `.claude/settings.json` is a single file-level symlink to `aida-core/templates/settings.json`.
- `.claude/commands/` is a regular directory whose **files** are per-file symlinks into `aida-core/templates/commands/` — managed by `make sync-templates`.
- `.claude/skills/` scaffolds (and this repo dogfoods) the **directory form**: `.claude/skills/<name>/SKILL.md`, not a flat `<name>.md`. Antigravity CLI 1.2.2 only recognizes skills laid out that way — a flat file directly under `.claude/skills/` is invisible to it, so only the matching `.claude/commands/<name>.md` twin showed up, rendered as an ugly `/source-command-<name>` (BUG-1135). Claude Code accepts both forms, so the dir form is the one true shape. Masters stay **flat** at `aida-core/templates/skills/<name>.md` — `make sync-templates` links each one to `.claude/skills/<name>/SKILL.md` (a skill that is already multi-file, e.g. `aida-pr`, links its whole master directory instead). The scaffolder normalizes this centrally in `create_artifact` (`aida-core/src/scaffolding/mod.rs`, `normalize_claude_skill_path`), so every downstream `aida init`/`aida scaffold apply`/`aida init --refresh` gets the fix, not just this repo.
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
- `--status`: `draft`, `approved`, `planned`, `in-progress`, `done`, `completed`, `rejected`, `superseded`
  - The full state machine — Draft → Approved → Planned → In Progress → Done → Completed → Released, with the precise verb for each transition and the edge cases (cluster PRs, parallel pipelining, autonomous drains) — is documented in `docs/lifecycle.md` and the README's "Spec lifecycle" section. trace:TASK-273
  - **`rejected` vs `superseded` (TASK-1176)**: the two terminal off-ramps mean opposite things. `rejected` = **declined** (we said no, nothing was adopted). `superseded` = **adopted, then replaced** (this spec governed; a later spec now does). Record the successor with `aida edit <ID> --status superseded --superseded-by <NEW-ID>` — the flag alone implies the status, and it writes a **first-class typed relationship** (`SupersededBy` + the reciprocal `Supersedes` on the successor), walkable by `aida graph` / `query_graph`, not a `superseded-by:ADR-N` string tag. Archetype: an ADR accepted and later replaced, which previously had to masquerade as Rejected. `superseded` is a **general** status (any type may use it; an EPIC is excluded only because its status is a read-only rollup) and is **terminal** — excluded from the default `aida list` open lens, the `aida status` open tally, and every candidate/ready surface, and a superseded child is *resolved* in the epic rollup exactly like a rejected one. It renders `⊡ Superseded` in dimmed closed-green — never the red `✗ Rejected`. trace:TASK-1176
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
  - `decision` (`ADR-N`) — Architecture Decision Record: a recorded decision + its rationale (stateful: proposed / accepted / superseded / deprecated; in AIDA statuses `draft` = proposed and `approved` = accepted — the advisor records acceptance with `aida edit ADR-N --status approved`, and `--status accepted` is an input alias for it). An accepted ADR is **terminal**: it renders as `☑ Accepted` (not the task-style `▸ Approved`) and the default open lens drops it from `aida list` and the `aida status` open tally — no manual archive needed. `aida list --all` / `--type decision` / any explicit `--status` still show it. trace:BUG-781 An ADR later replaced by a successor moves to the **`superseded`** status — `aida edit ADR-N --status superseded --superseded-by ADR-M` — which renders `⊡ Superseded` and records the successor as a typed `superseded-by` edge. trace:TASK-1176
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


## Migrated Lessons

### feedback_dogfood_config_assertions_read_at_runtime

When a test asserts a fact about **this repository's own** `.aida/config.toml` (a job stays
registered, a route stays bound), read the file at runtime:

```rust
let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
    .parent().expect("aida-cli-lib has a workspace parent");
let cfg = load_config(repo_root).unwrap().unwrap();
```

That is STORY-1423's documented idiom (`this_repo_keeps_the_performance_guard_pair_registered_and_enabled`,
~maintenance_schedule.rs:5209). Codex reached for `include_str!("../../.aida/config.toml")` instead
on BUG-1746 and I sent it back.

**Why:** `include_str!` is compile-time, so it makes the crate's test target fail to *compile*
without a repo-root file that is not part of the crate — a vendored `aida-cli-lib` stops building.
It also bypasses `load_config`, the loader the product actually uses, and it makes the one
mutation proof that matters impossible: unbinding a route in the real config cannot fail a test
holding a compiled-in copy. Two idioms for one job is also how the duplicate worktree-placement
rule TASK-1561 removed came to exist.

**How to apply:** grep for `include_str!` of any path outside the crate in a review. A template
under `aida-core/` is fine (workspace crate); repo-root operator state is not. Demand the mutation
proof that distinguishes them — edit the real config, expect the test to fail.

Related: [[feedback_prove_a_test_fails_without_the_fix]],
[[feedback_verify_acceptance_matches_primary_caller]],
[[feedback_source_scanning_guards_need_the_full_suite]].

### feedback_source_scanning_guards_need_the_full_suite

AIDA's test suite contains **architecture guards that scan the repository's own source text**,
not just behaviour — e.g. `story_1418_completion_seam_tests::no_direct_completed_write_outside_the_seam`
counts occurrences of `RequirementStatus::Completed;` and `set_status_from_str("completed")` per
file against an allowlist, and `every_file_that_stamps_completed_through_the_seam_also_emits`.
They live under a test name that shares NO substring with the code you changed.

On 2026-09-28 (BUG-1721) a codex implementer ran `cargo test -p aida-cli-lib --lib closure` and
`--lib auto_bump`, both green, and self-reported success. The full `--lib` suite then failed on the
seam guard: its fix mutated a store clone with `set_status_from_str("Completed")` to model
in-pass completions. The behaviour was right; it violated the "only `crate::completion` reaches
Completed" architecture. The fix was redesigned around a resolved-id set threaded into
`pickability::unresolved_closure_blockers_treating_resolved`, which also removed the store clone.

**Why:** a filtered test run is selected by NAME. A guard that indicts your diff by *scanning it*
is named after the invariant it protects, not after your subsystem, so no plausible filter reaches
it. The temptation when a guard fires is to widen its allowlist — that blinds it permanently.

**How to apply:** the integration seat runs the FULL `cargo test -p aida-cli-lib --lib` before any
commit, even when the implementer reported green — this is exactly the "full suite ONCE at
integration" half of [[feedback_dont_brief_full_workspace_suite_per_agent]]. When a source-scanning
guard fires, redesign to satisfy it honestly; never add your file to its allowlist, and never route
around the literal it greps for. See also [[feedback_prove_a_test_fails_without_the_fix]].

### project_aida_hub_is_first_user_dogfood_vehicle

`~/ai/aida-hub` (private repo `joemooney/aida-hub`, started 2026-08-23) is a project
catalogue — scans `~/ai` + `~` strays + remote GitHub repos, serves a Leptos 0.8 + axum
dashboard on port **8092**. Stack chosen to match `gld`/`aida-chat` so no new framework
enters the rotation.

**Why it matters to AIDA:** it is the first project bootstrapped from *nothing* with a
current `aida` build in a long time, so it doubles as the **first-user experience probe**.
Bootstrapping it surfaced four traps in one sitting, filed as STORY-780 (fold project
bootstrap into `aida init` — NOT `aida new`, that verb is reserved for higher-value uses)
and BUG-789 (`aida init` pushes the store branch but silently strands its own scaffold
commit on main).

**How to apply:** when touching AIDA's onboarding, scaffolding, or `init` surface, walk it
in aida-hub or a fresh throwaway rather than reasoning from the aida repo — the aida repo
is long-initialized and hides every first-run defect. File friction found there against
AIDA, not against aida-hub. See [[feedback-scripting-friction-is-a-missing-surface-signal]]
and [[feedback-self-test-via-dogfood-merge]].

### project_axi_incorporation_and_mcp_reweighting

2026-06-28 review of the **AXI ecosystem** (kunchenguid: axi/no-mistakes/gnhf/firstmate/treehouse/tasks-axi/gh-axi/lavish-axi) produced a positioning doc (`docs/positioning/vs-axi.md`), a competitive note (`docs/competitive-analysis/2026-06-28-axi-ecosystem.md`), and **EPIC-56** (the lessons backlog). A solo run shipped **14 lessons**: TOON agent-output renderer (TASK-964, measured 21-84% token cut, agent-mode-gated — emoji human path untouched), content-first/list-cap (TASK-970), structured-errors-on-stdout (TASK-972), lifecycle-aware next-step help (TASK-974), SessionStart-hook fix (TASK-971); reliability scar-closers patch-id force-push guard (BUG-640), worktree-tangle gate (TASK-965), config-trust-boundary RCE fix (TASK-969), process-group reaping (TASK-298); drain caps/exit-summary/idle-CI (TASK-966/967/968); the benchmark harness (SPIKE-73, `bench/agent-surface/`) + findings-list clarity (BUG-641).

**The load-bearing finding (SPIKE-73):** on AIDA's own surfaces, **MCP costs ~2x the tokens/dollars of the CLI for identical (or worse) success.** This challenges the README calling MCP "the highest-leverage surface." Direction: **token-efficient CLI (`AIDA_AGENT_OUTPUT`/TOON) as the PRIMARY agent surface, MCP as the typed/structural option.**

**RESOLVED (2026-06-29 full 72-cell matrix, 4 conditions × 6 tasks × 3 runs):** the ToolSearch-variant re-run that was the open caveat is DONE and CONFIRMS the result is STRUCTURAL, not just the upfront-schema tax. Per-condition: **cli 100% / $0.0358 / 3.1 turns; mcp 89% / $0.0709 (~2x) ; mcp-toolsearch 100% / $0.0636 (~1.8x) / 4.8 turns / 3.8 tools; toon 100% / $0.0360.** On-demand schema loading (mcp-toolsearch) does NOT rescue MCP — still ~1.8x CLI AND costs MORE turns/tools (the schema-load round-trips eat the input-token savings). So MCP is expensive structurally, not merely from loading 69 schemas upfront. TOON ≈ CLI here because the tasks are small-output reads (TOON's win needs the large list/show outputs — measured 21-84% there separately). Report: `~/ai/aida-spike73-bench/bench/agent-surface/results/report.md`. **The MCP-README-reframe is now evidence-backed.**

**Agent-output mode invariant:** all AXI interface changes gate on `agent_output_mode()` (non-TTY OR `AIDA_AGENT_OUTPUT`); the human TTY emoji path stays byte-identical (operator likes emoji — [[user_likes_emoji]]).

**PARKED for operator (architecture/keystone, never shipped unattended):** TASK-975 (CI-auto-fix loop), STORY-712 (event-driven zero-token supervision — firstmate's biggest lever), STORY-713 (single-liaison dispatcher), SPIKE-74 (agent-agnostic drain backend), STORY-714 (worktree warm-pool BUILD — design doc landed, Slice-1 awaiting sign-off; dissolves TASK-0396 + BUG-553). STORY-715 (tmux ambient meter) deferred behind EPIC-53. Open operator decisions: warm-pool Slice-1 sign-off, MCP README re-frame, the ToolSearch benchmark re-run.

### project_bugs_before_marketing_phase

Operator's stated priority phase (2026-05-29): **clear out all known bugs → achieve stability → THEN begin to consider marketing.**

**Why:** recording a public demo / autonomy-keystone narrative now (e.g. TASK-403) would showcase the very bugs being cleared. The `aida --asciinema` cast-capture tooling already exists, so demo recording is NOT blocked on tooling — it's gated on the system being stable enough to be worth showing. Marketing before stability is premature and counterproductive.

**How to apply:**
- **Prioritize bug-clearing work** (findings → BUG/TASK fixes, the open `aida findings list` backlog, reliability fixes) over net-new features, demos, and visibility/marketing tasks.
- **Defer or reject `marketing` / `demo` / `visibility` / `wedge`-tagged work** until the bug backlog is cleared. TASK-403 (record overnight keystone drain) was rejected on these grounds — revive/re-file once bugs are cleared and the keystone runs clean.
- When proposing "do more," lean toward **bugs and stability**, not features or marketing. The orchestrator-reliability majors (TASK-136 drain stall, TASK-137 reviewer anchoring, TASK-133 phase-1 lease ordering) and correctness bugs (TASK-132 list multi-filter OR-not-AND) are on-phase; new compose SPIKEs and demos are off-phase for now.
- This is a *phase*, not permanent — when the operator says stability is reached, marketing re-enters scope (the `--asciinema` tooling is ready for it).

Connects to [[feedback_capture_vs_slop_in_article_flow]] (ship selectively) and [[feedback_pushback_on_overengineering]] (scope discipline). Reinforces that the current bar for "do more" is *fix what's broken*, not *add or promote*.

### project_burst_usage_robustness_over_speed

The operator (2026-09-11) works in **burst-file-then-quiet** cycles, with long gaps of no filing. So the backlog is NOT continuously growing — it SITS STUCK. The pain is that draining/closing open items is unreliable: a single tooling hiccup parks a spec (NeedsAttention) and it sits for DAYS because nothing re-drives it and the operator isn't watching.

**Why:** I mis-framed this once as "intake 3.6x merge rate → filing too fast." That ratio is real over a burst window but MEANINGLESS for this usage — the operator isn't sustaining that intake. The binding constraint is drain robustness, not throughput. Correcting this stops a future session from chasing speed (self-hosted runner, backpressure) when the real problem is that the automation can't run unattended.

**How to apply:** For "the drain is slow / stuck," reach for RELIABILITY first (does it self-recover a transient park? does it need a human to relaunch?), not speed. Speed levers (STORY-1049 self-hosted runner = opt-in toggle only; STORY-1050 backpressure = deferred, wrong usage pattern) are secondary. The reliability effort is [[project-epic62-unattended-drain-reliability]]. See also [[feedback-substrate-first-never-rely-on-agent-awake]].

### project_champion_product_not_probe

**Operator, 2026-08-29:** *"forget about the research angle and get into the mindset of making this
a champion product… I want a relentless push forward."*

This **supersedes** [[project_aida_is_a_probe_not_the_objective]]. The probe framing (AIDA as
existence-proof, "the honest answer may be don't build this") is retired as the governing register.
Build, position and prioritise AIDA as a product intended to win.

**What changes:**
- Prioritise by *user value and adoption*, not by what the research question would find interesting.
- Product-defensibility claims are back on the table — but [[feedback_precise_claim_not_overclaim_in_positioning]]
  still holds. Champion ≠ overclaim; a product that oversells is a worse product.
- Refresh the market analysis on a real cadence, not as a one-off
  ([[feedback_competitive_analysis_is_living_doc]]).
- Bias to shipping. Fewer spikes, more slices that a user can feel.

**What does NOT change:**
- The public-repo confidentiality rule ([[feedback_public_repo_scrub_employer_content]]) — still
  absolute. Frame vendor-mandate context generically; never tie it to the operator's employer.
  *(Violated once on 2026-08-29 when EPIC-60/TASK-1182 named it; scrubbed forward, but the orphan
  store's git history still carries the original text.)*
- Truthfulness over advocacy in findings. Report what is, then argue for the product.

**Live market signal to build on:** vendor-standardization mandates are real — organisations are
picking one coding-agent vendor and prohibiting others. That makes cross-vendor durability
(AIDA's actual niche) a *purchase reason*, not just a research finding. See [[project_forge_plumbed_not_wired]]
and the Codex parity epic.

### project_forge_plumbed_not_wired

EPIC-35 forge-provider routing status (updated 2026-06-05, keyboard session with operator):

- **Plumbed (slice 1):** `Forge` trait, `ForgeKind`, config + origin auto-detection, `GitHubForge` (real gh), `PureGitForge` (real, squash-commit-fixed), forge-aware hint/error TEXT (STORY-508). `GitLabForge` open_change real; change_status/ci_status/change_for_branch are stubs.
- **WIRED so far — the MERGE op (STORY-516 slice-1b, first op of SPIKE-49 order):**
  - **TASK-668** routed the `aida pr ship` step-3 merge through `forge_for(root).merge_change()`. GitHubForge::merge_change reuses the SPEC-410-pinned `pr_ship::merge_args` (argv byte-identical) + network_retry wrapper.
  - **TASK-669** routed the orchestrator auto-complete phase-4 merge. Blocker was the orchestrator's DualSink (stderr + drain-state, BUG-286) — resolved by adding `sink: &mut dyn network_retry::RetrySink` to `Forge::merge_change` (RetrySink/RetryEvent are now `pub`). gh-missing keeps a non-shelvable MissingTool pre-check; failed merge → shelvable `Failed` PhaseFailure (EPIC-28 classification preserved).
  - **Unified contract:** all three providers' `merge_change` now return `Err(stderr)` on failure (was `Ok{merged:false}`).
  - **Validated the strongest way:** PR-536 (TASK-668) and PR-537 (TASK-669) were each merged via `aida pr ship` (the routed `merge_change`) — i.e. the routing merged its own PRs on real GitHub PRs.
- **WIRED — the VIEW op (PR-538, 2026-06-05):** `Forge::change_for_branch` enriched from `Option<ChangeRef>` to a 5-state `ChangeLookup` (Found/NoChange/CliMissing/CliFailed/Unreachable) preserving the BUG-257 transient-vs-definitive distinction; `ChangeRef` gained `title: Option<String>`. GitHubForge delegates to the proven `detect_open_pr_for_branch` + a pure tested `change_lookup_from_pr_lookup` mapping (no classification reimpl). All 10 `detect_open_pr_for_branch` callers migrated to the forge-routed `change_lookup_for_branch` helper — **including the two BUG-444/BUG-257 phase-1 sites** (`detect_phase1_pr`, resumed-implementer), contract preserved exactly. Dogfood-validated (PR-538 merged via `aida pr ship`, exercising step-1 resolution through the new path). 1614 tests green. NOTE: PR-538 trailered STORY-516 directly (a child-TASK slip vs the merge op's TASK-668/669) — STORY-516 stayed Approved, not mis-bumped, but watch a future `aida pull` and reopen with --force if it auto-completes.
- **WIRED — the CI POINT-PROBE (TASK-672, PR-539, 2026-06-05):** new forge-owned `CiProbeResult` (NoSignal/NoChecks/InProgress/Green/Failed) mirroring `CiProbe`; `Forge::ci_probe_for_branch` delegates to the proven `probe_ci_state_for_branch` + pure tested `ci_probe_result_from_ci_probe`. crate helper `ci_probe_via_forge(branch)` converts back to CiProbe so the 6 call sites (incl. orchestrator CI phase) are a pure name-swap; project_root resolved internally (only selects provider; gh runs in CWD via delegate). Dogfood-validated. 1615 tests green.
- **STILL NOT wired:** the CI **WATCH** ops — `gh pr checks --watch` (pr ship step 2 + orchestrator CI watch) + `gh run watch` (`--watch-ci`) + `gh run view` (forge models a point query, not a blocking watch/stream — needs a watch method or keep-loop-route-per-poll); `gh pr create`; `gh pr list` (`detect_open_pr_for_spec` spec-search + `detect_merged_pr_for_branch` --state merged); `gh pr comment`; `gh pr checkout`. STORY-516 stays **open**.

**Pattern that works (proven 3×):** forge-owned result type mirroring the rich main.rs type (ChangeLookup←PrLookup, CiProbeResult←CiProbe) + GitHubForge DELEGATES to the existing battle-tested helper + pure tested mapping fn + crate helper that callers swap to. Zero classification reimpl → contracts preserved. **TRAILER A CHILD TASK, never (STORY-516) directly** — the umbrella has now been auto-mis-bumped to Completed TWICE by direct-umbrella trailers (PR-483 historically, PR-538 on 2026-06-05); reopen with `aida edit STORY-516 --status approved --force` if it happens again.

**How to continue:** merge ✅ → view ✅ → ci-probe ✅ → ci-watch[pr-ship] ✅ → create ✅ (TASK-675) → comment ✅ (TASK-676, delegated-review trigger→forge.comment) → checkout ✅ (no live `gh pr checkout` sites — only hint text + git checkout in tests) → **remaining = STORY-521 (only 2 ops left)**: (1) **list** — `detect_open_pr_for_spec` (gh pr list --search) + `detect_merged_pr_for_branch` (gh pr list --state merged), both return PrLookup → reuse the change_lookup pattern; ~9 callers across main.rs; and (2) **orchestrator ci-watch** — `watch_ci_for_context`/`gh run watch` streaming + the non-streaming poll-wait + headless variants + `gh run view` (the one genuinely complex op; needs a forge watch/stream method beyond the pr-ship `gh pr checks --watch` already done). Each: own PR, child-TASK trailer parented to STORY-521, dogfood via `aida pr ship`, reliability-critical → keyboard.

**Tracking note:** STORY-516 is stuck Completed (its own PR-538 commit permanently trailers `(STORY-516)`, so every `aida pull` re-bumps it — reopening is futile until that commit ages out of the scan window). The durable open tracker for the remainder is **STORY-521** (child of EPIC-35, referenced by no commit). When filing the remaining-op child TASKs, parent them to STORY-521, not STORY-516. Keep GitHub argv byte-identical; for any op with a retry/sink or typed-failure contract (like the orchestrator merge had), preserve it. Reliability-critical → keyboard, watched, per [[feedback_reliability_fixes_use_keyboard_not_drain]]; dogfood each by shipping its own PR through the routed path. After routing: STORY-509 (GitLab glab impls), STORY-510 (GitLab CI), STORY-511 (e2e drain + linkage + docs).

History: STORY-516 was once mis-bumped Completed by PR-483's trailer (only prep landed) — reopened 2026-06-05. Classic [[feedback_commit_trailer_completes_the_spec]] violation; child TASKs (668/669) carry the real routing trailers, umbrella stays open.

### project_main_branch_protection_requires_only_merge_hold_gate

History: until 2026-09-17 `required_status_checks.contexts` was `["merge-hold-gate"]` only — `Build (ubuntu-latest)` (PR CI) was NOT required, so any required-only CI semantics (`gh pr checks --required`) would have treated a red Linux build as ignorable. On 2026-09-17 the operator agreed and I added it: contexts are now `["merge-hold-gate", "Build (ubuntu-latest)"]`, `strict = false` (verified by read-back via `gh api repos/joemooney/aida/branches/main/protection/required_status_checks`).

TASK-1205's informational Windows/macOS matrix jobs are also named `Build (windows-latest)` / `Build (macos-latest)` but live in the `Cross-platform` workflow — BUG-1180 (ADR-39) classifies red checks with the `[ci] informational_workflows` allow-list (default `["Cross-platform*"]`) so they never block.

**How to apply:**
- Merge automation may key on the required set now, but still keep the informational allow-list — the matrix jobs are not required and must stay ignorable.
- `gh pr checks --json name,bucket,workflow` is the reliable per-check surface; `bucket` ∈ pass|fail|pending|skipping|cancel; gh exits non-zero on fail/pending but still prints JSON.
- GitLab mirror analog (TASK-1254): project `ai/aida` has "Pipelines must succeed" on and the `aida-merge-hold-gate` job included from the scaffolded template; see [[reference_gitlab_mirror_runner_setup]].

Related: [[feedback_verify_ci_green_before_merge]], [[feedback_ci_surface_beyond_cargo_test]].

### project_repositioning_parked_behind_stabilization

The AIDA repositioning effort (ADR-4 + EPIC-39 + 9 child slices + SPIKE-58/59) is **fully planned, red-teamed, and PARKED** as of 2026-06-12. Operator: *"I am going to try to stabilize aida before any pivot to bd/gt."* This extends [[project_bugs_before_marketing_phase]] (2026-05-29: bugs → stability → then outward moves).

**State on park:**
- Branch `repositioning` (worktree `~/ai/aida-repositioning`, pushed, 7 commits): the plan (`docs/plans/2026-06-12-repositioning-governance-layer.md`, §13 = red-team amendments), the rescope + ERRATUM, all evidence.
- ADR-4 = **PROPOSED, not accepted** — the SPIKE-58 hands-on bd/gt gate was deferred, not resolved. The canonical wedge (operator's words, on ADR-4): *AIDA governs at the FRONT (programmatic pre-work approval gate + code↔spec bind); bd/GT govern at the back/side (reactive escalation + merge gates), no pre-work gate, no spec↔code binding.* Anchored on Yegge's verified "Beads is an execution tool" quote.
- All 11 family specs carry `deferred:stabilization-first` (parking tag — the burndown pickability gate excludes them; intake/groom passes must not queue them).

**Reopen trigger:** operator declares stabilization done. **On reopen, first action:** re-verify the competitor snapshots (Beads moves fast — stars/releases/features were stale within days twice during planning); then resume at SPIKE-58 (hands-on gate) → WS1 copy lock.

**Why a memory:** cross-session state not derivable from any single artifact — prevents a future advisor/intake pass from grooming the parked family or treating ADR-4 as accepted.

### project_scaffold_upgrade_corrupts_dev_repo

**FIXED (BUG-718, merged ad432cdf0 on 2026-07-11):** `scaffolding::symlink_target()` now guards all three write paths (`apply_with_options`, the `scaffold apply` loop, `run_scaffold_upgrade`) — a symlinked scaffold file is skipped + reported ("NOT written"), never written through. The in-repo build is safe. **Caveat:** any pre-fix binary still corrupts — the released `aida` on PATH (0.9.1) does NOT have the guard, so run the in-repo build (`aida dev activate` / `./target/*/aida`) when scaffolding in this repo until a release ships with the fix.

Mechanism (why it happened): in the AIDA dev repo, `.claude/skills/*`, `.claude/commands/*`, `.claude/hooks/*`, `.claude/settings.json` are per-file **symlinks into `aida-core/templates/`** (the master copies this repo dogfoods). Pre-fix, `aida scaffold upgrade` / `apply` classified those as *template category = overwrite-on-drift*, followed the symlink, and wrote the embedded template **through the symlink into the source master** — silently corrupting `aida-core/templates/`.

**Incident 2026-07-11:** a loop agent (or stray process) ran a scaffold upgrade in the MAIN worktree. It gutted `aida-pre-commit.sh` 275→68 lines, deleted 476 lines of `docs/agents/`, regenerated every command/skill master into output-format ("AIDA Generated: v2.0.0" headers on the *masters*), and created `.claude/AIDA.md` + `.codex/`. It also broke the scaffolding tests (the gutted hook stopped rejecting `///` trace markers) — which masqueraded as a mysterious test regression.

**Recognize it:** many tracked files modified at one uniform mtime; "AIDA Generated" checksum headers appearing on the *master* templates in `aida-core/templates/` (masters should have `--- description: --- ` frontmatter, not the generated header); new untracked `.claude/AIDA.md` / `.codex/` / `.claude/skills/local/`.

**Recover:** every change is content *loss* vs HEAD, so `git checkout -- aida-core/templates/ .mcp.json CLAUDE.md docs/agents/ …` restores the good versions (fully recoverable), then remove the errant untracked strays.

Also see BUG-719 (a stale binary's embedded templates resurrect deliberately-deleted scaffold files — that's how the removed `aida-recover` skill reappeared). Related: [[feedback_fan_committing_agents_with_worktree_isolation]] — the same isolation-violation footprint (an agent acting in the main worktree). Fix tracked in BUG-718.

### project_single_drain_lock_per_repo

AIDA enforces **one drain per repo** via a GLOBAL lock at `.aida/drain.lock` (`aida-cli/src/drain_lock.rs`, BUG-538). Any drain entry point (`aida zen`, `aida queue work --auto-complete`, `aida burndown run`) acquires it; a second one while the first is live is **refused** with "a drain is already running (pid …)". Per-scope/per-worktree parallel drains are *explicitly out of scope* in the code — even `--solo` drives (own worktree/branch/PR) share the one global lock.

**So concurrent `aida zen --solo` invocations do NOT run in parallel** — despite `aida zen --help` saying "fire several for INDEPENDENT specs in parallel and walk away." That help text overclaims (filed BUG-682). The real parallelism mechanism is the **burndown fan-out**: ONE orchestrator holds the single lock and fans out N worktree-isolated implementer subagents (`aida burndown` / `aida queue work --batch --auto-complete`). To run multiple specs concurrently, queue them + run one burndown — do not launch multiple zen drives.

Do NOT `AIDA_DRAIN_FORCE=1` past a LIVE drain — that bypass is only for a dead/stale lock; forcing past a live one causes the double-drive (two integrators racing git ops on main) the lock exists to prevent.

**Why:** I told the operator to fire 3 `aida zen --solo` drives in parallel based on the help text; the global lock refused the 2nd and 3rd (they silently never started). Trusted the doc over the code — see [[feedback_verify_lore_against_code_not_docs]]. The BUG-538 "revisit if real demand for concurrent non-overlapping drains appears" trigger has now fired → STORY-746 (deferred).

### reference_build_slots_sccache_mold

Installed 2026-09-26 at Joe's request to raise throughput.

- `~/.local/bin/cargo` (ahead of ~/.cargo/bin on PATH) gates build/test/check/clippy/run/bench/doc/install on `AIDA_CARGO_SLOTS` (default 2) flock slots under `$XDG_RUNTIME_DIR/cargo-slots-<uid>/`; defaults CARGO_BUILD_JOBS=3; nested cargo passes through (AIDA_CARGO_SLOT_HELD); bypass with AIDA_CARGO_NO_SLOT=1. Prints "cargo: waiting for a build slot" while queued.
- `~/.cargo/config.toml`: rustc-wrapper = sccache; linker clang + `-fuse-ld=mold`. `sccache --show-stats` for hit rate.
- Consequence: the orchestrator no longer needs to count cargo processes by hand — dispatch freely; builds queue. Tell agents to use long timeouts (3600s) because of queueing.
- Supersedes the manual "at most 2-3 cargo processes" rule in [[feedback_cap_parallel_cargo_builds]] (the cap is now enforced).
- Product follow-up: AIDA-owned build slots (substrate-as-bouncer).

**Fable weekly cap (2026-09-26):** `claude-fable-5-1` hit a weekly usage limit at 17:26 Sat (reset 07:00 Sun) and killed two subagents mid-run (uncommitted work left in worktrees). Use Fable for the highest-value strict reviews only; have implementers commit WIP before long test chains.

### reference_codegraph_local_setup_and_1mib_cap

Set up 2026-10-02 at the operator's request (he vouched for codegraph+aida coexistence, SPIKE-84).

- Index: `.codegraph/codegraph.db` in /home/joe/ai/aida (gitignored). Rebuild: `codegraph index .`
- Freshness: systemd user timer `codegraph-sync-aida.timer` runs `codegraph sync --quiet` every 15 min (`systemctl --user list-timers | grep codegraph`). A git post-merge hook was rejected because the repo's post-merge is a symlink that `make install-agent-skill-hooks` clobbers.
- Surface: CLI only (`codegraph callers/callees/impact/query`), never MCP — the SPIKE-73 benchmark made CLI the primary agent surface ([[project_axi_incorporation_and_mcp_reweighting]]).

**The trap:** codegraph v1.6.0 hardcodes `MAX_FILE_SIZE = 1 MiB` (non-configurable const in `~/.codegraph/versions/v1.6.0/lib/dist/extraction/index.js`). `aida-cli-lib/src/lib.rs` is 114k lines / 4.7 MB, so it indexes with **0 symbols**: `callers`/`impact` silently omit every caller in lib.rs, and symbols defined there (e.g. `agent_output_mode`) don't resolve at all — the query fuzzy-matches similarly-named test fns instead, which looks like a real answer. Treat "no callers" as unverified until `rg` agrees. Documented in CLAUDE.md's read-on-demand table (TASK-1568, PR #2344).

Also: `codegraph init/index` can exit 0 while printing `Failed: database is locked` — verify with `codegraph status` (it reports a truncated index explicitly), never the exit code.

### reference_codex_exec_dispatch_recipe

Joe authorized codex for the reviewer and implementer seats on 2026-09-27. It is the main lever
against the seat-cost problem in [[feedback_serial_not_fanout_on_this_host]]: a codex review costs
the orchestrator a few hundred tokens instead of ~100k, and it is genuine cross-vendor independence
on the reviewer seat.

**The recipe:**

```bash
codex exec -C <worktree> --sandbox workspace-write \
  -o <verdict-file> "$(cat <brief-file>)" \
  < /dev/null > <run-log> 2>&1
```

Run it as a background Bash job. `-o` writes ONLY codex's final message, so its transcript never
enters the orchestrator's context — read the `-o` file, never the run log or the task output file.

**`< /dev/null` is mandatory and non-obvious.** Without it a backgrounded `codex exec` prints
`Reading additional input from stdin...` and **hangs forever** — stdin is an open pipe that never
reaches EOF, so it never processes the prompt. The run log sits at 39 bytes and the verdict file
stays empty. On 2026-09-27 two dispatches (a BUG-1677 rework and a review) hung ~25 minutes this
way while I believed they were working; an earlier call happened to get a closed stdin and ran
fine, which masked the bug. **Check that the `-o` file is non-empty before trusting a "completed"
job**, and if a codex job finishes suspiciously fast or slow, read the run log's size first.

**Other things learned:**
- `codex review` exists for review-shaped runs; `aida agent new codex` spawns with project-correct
  cwd/env and registry tracking.
- Under `--sandbox workspace-write` the `aida` CLI may fail to enable WAL mode on its cache, so
  codex cannot run `aida show`. Tell it to read the canonical spec object from `.aida-store`
  directly. Its configured profile already grants write access to `~/.cargo` and `~/.rustup`, so
  cargo builds work.
- A codex verdict is only as good as what it ran. It will say plainly whether it ran tests — a
  static-reading review is still valuable but is NOT test-backed evidence. Verify any blocker it
  reports yourself before acting; on BUG-1677 its blocker was precise and correct.
- Codex does not read the aida mailbox — see [[feedback_no_mail_storm_to_codex]].

**Brief LENGTH decides whether you get a verdict at all (2026-09-27).** A ~50-line review brief that
invited the agent to "inspect" and judge several risk areas produced a 626-line run log of the agent
reading docs and then **NO final message and an empty `-o` file** (exit 0, so the job looked fine).
Re-dispatching the same review as a tight brief — naming the exact commands to run, forbidding repo
exploration ("do not read CLAUDE.md/AGENTS.md/docs"), capping the answer ("under 300 words"), and
giving an exact reply template — returned a clean verdict every time, on three separate reviews.
Budget the agent's attention, not just its sandbox: say what to read, what NOT to read, and the shape
of the answer. `--sandbox read-only` is enough for a review and avoids the cargo-slot problem
entirely.

**It is worth it: cross-vendor review caught real bugs in my own code three times in one session** —
a lexical `Path::starts_with` that made a doctor check cry wolf on an already-trusted directory, a
`?` that turned a fail-open guard into a fleet-wide launch abort, and a wrong-type TOML read that
silently defaulted `[agents] mcp` to `off` against its own documented contract. None were visible
from my own reading of the diff. See [[feedback_proxy_reviewer_with_independence_rule]].

**Do NOT add `nohup` or a trailing `&` inside a `run_in_background` Bash call (2026-09-28).** Run
`codex exec` in the FOREGROUND of the backgrounded tool call. With `nohup ... &` the wrapper shell
exits instantly, the harness reports **"completed (exit code 0)" in under a minute**, and the codex
keeps running **untracked**. On BUG-1720 that false completion made me conclude the run had died —
`git diff` was empty because codex was still in its reading phase — so I re-dispatched, and had **two
codex processes editing the same worktree and writing the same `-o` path** for nine minutes. Both had
to be killed and the worktree reset; ~20 minutes lost.

Diagnostics that would have caught it sooner: a "completed" dispatch whose `-o` file is absent but
whose **run log keeps growing** is alive, not dead — `ls -l` the log twice. `ps -eo pid,lstart,etime,args
| grep '[c]odex exec'` shows every live dispatch with its start time; use it before re-dispatching
anything. And note that grep will also match **this session's own `claude -p` relay process**, because
the handoff text quotes the recipe — see the pgrep-matches-own-commandline note in the handoff.

A worktree two concurrent agents scribbled in cannot produce trustworthy evidence, even if the final
diff looks plausible: each agent's test runs were made against a state the other was mutating. Kill
both, `git checkout --` the touched files, dispatch once. Same family as "do not diff a worktree while
the dispatched agent is alive."

### reference_codex_needs_add_dir_for_cargo_slots

`codex exec --sandbox workspace-write` cannot take the machine-wide cargo build
slot, so it cannot run `cargo test`/`cargo build` through `~/.local/bin/cargo`
at all. The wrapper's slot dir is `${XDG_RUNTIME_DIR:-/tmp}/cargo-slots-$(id -u)`
= **`/run/user/1000/cargo-slots-1000`**, which is outside the sandbox's writable
roots, so `flock` fails and the wrapper emits repeated lock errors instead of
queueing. Observed 2026-09-27 on both the STORY-1478 and TASK-1517 seats: the
build succeeded (it only needs the workspace) but every targeted test run died
on the slot gate.

Fix: add the slot dir as a writable root. The flag is **`--add-dir`**:

```bash
codex exec -C <worktree> --sandbox workspace-write \
  --add-dir /run/user/1000/cargo-slots-1000 \
  -o <verdict-file> "$(cat <brief>)" < /dev/null > <runlog> 2>&1 &
```

**Why this matters beyond convenience:** this is the real reason a codex seat
once "bypassed the slot lock with `AIDA_CARGO_NO_SLOT=1`". That was not defiance
of the brief — it was the only way it could run cargo at all. Briefing harder
against the bypass (as [[feedback_cap_parallel_cargo_builds]] and the
build-slot rules push toward) makes it *worse*: the seat then either loops
retrying a lock it can never take, or reports tests it never ran. Either way the
result is unverified work that looks finished.

**How to apply:** always pass `--add-dir /run/user/1000/cargo-slots-1000` to any
codex seat expected to build or test. If a codex seat reports "could not acquire
its required slot" or "lock paths were read-only", that is this bug, not a
resource shortage — do not wait it out. And treat any codex test numbers as
unverified until the seat had that writable root: re-run the targeted tests from
an unsandboxed session before opening a PR. Related:
[[reference_codex_exec_dispatch_recipe]], [[reference_build_slots_sccache_mold]].

### reference_gh_job_logs_servable_mid_run

`gh run view <run> --job <id> --log-failed` gates on the **run** and refuses with *"run is still in progress; logs will be available when it is complete."* The jobs endpoint gates on the **job**:

```
export XDG_CACHE_HOME=<writable dir>     # gh writes a cache zip; a read-only
                                         # HOME fails with what looks like a
                                         # permissions error, not a cache error
gh api repos/<owner>/<repo>/actions/jobs/<job_id>/logs
```

That returns the full log of a **finished** job while sibling matrix legs are still running.

**The boundary, both sides pinned:** it gates on the **job's** completion, not the run's. A job still in progress returns **HTTP 404** — which means "this job has not finished", not "you cannot see this". So the win is that you never wait on sibling legs (macOS finishing after Windows); it is not that you can read a job mid-flight. Get `<job_id>` from `gh api repos/<o>/<r>/commits/<sha>/check-runs` or from the `detailsUrl` on a check.

**Why it matters:** this makes "identify a red before acting on it" free. A rule with an *imaginary* cost gets broken by someone reasonable — a matrix leg failing while macOS still runs looked like it forced a choice between waiting and merging, and it never did. The refusal was a property of the command chosen, not of the CI provider.

It is also another instance of asking a surface that doesn't own the concept: `gh run view` owns runs, not jobs. Same family as [[feedback_null_grep_for_invented_terms_is_not_absence]] and [[feedback_check_ignore_names_the_winner]].

Related: [[feedback_verify_ci_green_before_merge]], [[feedback_gh_run_rerun_is_not_fresh_ci]], [[feedback_read_the_verdict_not_just_the_check_rollup]].

### reference_git_push_dry_run_still_pushes_the_mirror

In `~/ai/aida` the git push wrapper mirrors refs to the `gitlab` hub, and **that step does not honour
`--dry-run`**. The origin half of the dry run behaves correctly (nothing is pushed); the mirror half
performs a real push.

Observed 2026-09-27:

```
$ git push --dry-run origin main
  mirrored 1 ref(s) → gitlab: main@4cf02628aeec     <-- actually pushed
To https://github.com/joemooney/aida.git
   2a55e91082..4cf02628ae  main -> main             <-- correctly only a preview
```

`git ls-remote gitlab main` then returned `4cf02628ae`. The real push to origin was afterwards
**rejected** by branch protection, so the dry run left gitlab holding a commit origin does not have —
it CREATED a mirror divergence while being used to avoid creating one. Filed as **BUG-1706**.

**How to apply:**
- Do not reach for `git push --dry-run` here to test whether a push is safe. It is not read-only.
- To learn whether protected `main` will accept something, just attempt the real push — GitHub
  rejects cleanly with `GH006: Protected branch update failed` and changes nothing.
- After any accidental dry run, check `git ls-remote gitlab <branch>` before assuming nothing moved.
- Repairing a mirror that got ahead needs a force-push to a shared branch, which AIDA's guidance and
  the git guardrail both refuse — so the cleanup is genuinely expensive. Avoid the trigger.

Related: [[feedback_serial_not_fanout_on_this_host]] for why mirror noise costs real time.

### reference_gitlab_mirror_runner_setup

Facts established 2026-09-18 while validating the GitLab drain (TASK-1254):

- **Project**: `ai/aida`, id 2, on `gitlab.joemooney.com` (root account via `glab auth`). Remotes in the dev repo: `gitlab` / `all` (all = gitlab fetch, gitlab+github push). GitLab-origin checkout for drain tests: `/home/joe/ai/aida-gitlab-mirror` (remote `github` added; local uncommitted config: `[review] mode = "delegated"`, `[store.sync] mirror_remotes = ["github"]`, node id 9). The older `aida-gitlab-clone` / `aida-gitlab-test` dirs point at the throwaway `joe/aida-gitlab-test` project.
- **Runner**: `imac-docker` (id 1, instance runner, untagged OK) runs on THIS machine as a root systemd service reading `/etc/gitlab-runner/config.toml`. `gitlab-runner register` run as joe writes `~/.gitlab-runner/config.toml` instead — the service then has no `[[runners]]` block and never polls (contacted_at frozen at registration). Fix = copy the block into the service config; it hot-reloads (`concurrent` too). `sudo -n` is passwordless here.
- **Docker is podman**: `/var/run/docker.sock -> /run/podman/podman.sock`, no `docker` group exists; the docker executor works because the service runs as root. `[runners.cache]` with empty Type logs a harmless "Could not create cache adapter" ERROR; the `/cache` volume still persists the job cache.
- **glab quirks**: `glab api --hostname H …` works; `glab mr …` has no `--hostname` — use `GITLAB_HOST=gitlab.joemooney.com glab mr create -R ai/aida …`. No `--jq`; pipe to python. `POST projects/2/pipelines/N/cancel` returns 403 with this token.
- **Pipeline**: `.gitlab-ci.yml` stages verify/test/release; `verify` is a cold cargo build+test on `rust:latest` (cache keyed on Cargo.lock since TASK-1254, was per-branch); gate job `aida-merge-hold-gate` included from `aida-core/templates/gitlab-ci-merge-hold-gate.yml` (self-contained alpine, `needs: []`). "Pipelines must succeed" is ON for the project. A push to mirror `main` triggers a branch pipeline that competes with MR pipelines for runner slots.
- **Store on two hubs**: mirror `aida-store` was 1921 commits behind until fast-forwarded 2026-09-18; a write from the mirror checkout fans out to github via its local `mirror_remotes`, and the dev repo fans out to gitlab — keep both in sync or `aida remote status` shows drift.

Related: [[project_main_branch_protection_requires_only_merge_hold_gate]], [[feedback_anchor_pgrep_patterns_monitor_shells_self_match]].

### reference_is_ancestor_lies_after_squash_merge

This repo merges with `gh pr merge --squash`, so a merged branch's tip is **never** an ancestor of
`main`. `git merge-base --is-ancestor bug-1752 main` returned NO for bug-1752, bug-1754 and bug-1755
— all three of which were merged that same hour (#2311, #2312, #2313).

The predicate that works:

```bash
gh pr list --head "$b" --state all --json number,state,mergedAt \
  -q '.[]|"#\(.number) \(.state) merged=\(.mergedAt)"'
```

**How to apply:** never decide "is this branch safe to reclaim / is this work shipped" from
`--is-ancestor` or from `git branch --merged` here. Ask the PR. This also matters for BUG-1756's
unshipped-work detector: a squash-merged branch and a genuinely-unshipped branch look identical to an
ancestry test, which is a plausible source of the OVER-reporting that family has been fixed for four
times. Related: [[feedback_fresh_branch_can_miss_interleaved_squash]],
[[reference_merged_spec_needs_queue_done_then_aida_pull]].

### reference_mail_intake_bug_reports

Joe sends AIDA bug reports from his work machine to his home Gmail (joe.mooney@gmail.com) with **subject containing "aida"**. The advisor session retrieves them with the Gmail connector (`search_threads` query like `subject:aida newer_than:7d`, then `get_thread` with PLAIN_TEXT) and files specs from them — established 2026-09-09, when the mailbox already held every report he had been pasting by hand.

**How to apply:** On session start (or when Joe says "check mail"), search for messages newer than the high-water mark in `.aida/mail-intake-highwater` (main repo, gitignored), file/annotate specs for anything new, then update the high-water file with the newest message timestamp and a one-line ledger. The Gmail connector is read-mostly — label writes fail on scope, so the local high-water file is the processed-state tracker. Work-report content is employer-adjacent: scrub identifying details (domains, internal model names, project names) before anything lands in the public repo's specs. GitHub CI notification mail also lands in this inbox — check for unread nightly cross-platform failures while there.

### reference_merge_hold_clear_is_human_only

Observed 2026-10-02 (PR #2339/BUG-1771): `aida merge-hold clear <pr>` refuses from any agent seat — "clearing a merge-hold requires a human at an interactive terminal; dispatch or advisor authority cannot override this integrity floor." This is by design (the hold is armed by a request-changes verdict via the BUG-1773/1774 verdict-corpus machinery) and is NOT lifted by operator proxy mode or --dangerously-skip-permissions.

**How to apply:**
- When a held PR's rework is approved, the closing paste for the operator is THREE steps, not two: `aida merge-hold clear <pr>` → `gh run rerun <failed merge-hold-gate run id>` (the gate check stays red from the pre-clear run; GitHub needs it green) → `gh pr merge <pr> --squash && aida pull`.
- Fetch the failed gate run id beforehand (`gh run list --branch <branch>`) so the paste is exact.
- Do not attempt PTY tricks or label edits to satisfy the gate — removing the label by hand does not release the hold (the gate reads recorded clearance), and forging the terminal check is exactly the BUG-1669-class shape the substrate exists to refuse.

Pairs with [[feedback_recording_a_verdict_blocks_your_own_merge]] and [[feedback_proxy_reviewer_with_independence_rule]].

### reference_merged_spec_needs_queue_done_then_aida_pull

Observed 2026-09-29: TASK-1532 had been merged a full session earlier and TASK-1559
merged this session, both with correct `(SPEC-ID)` commit trailers — and both still
read `in-progress`.

The missing steps, in order:

1. `aida queue done <ID> --yes` → status `done`
2. `aida pull` → the trailer auto-bump moves `Done` → `Completed`, printing
   `auto-bumped 2 specs → Completed` with the commit sha for each.

`git pull` does **not** trigger the auto-bump — only `aida pull` / `db sync --pull`
scans merged commits for `(SPEC-ID)` trailers. See
[[feedback_commit_trailer_completes_the_spec]] for what the trailer means and the
traps around putting an unfinished umbrella's id in one.

Then `aida remote reconcile --execute` to push the status change to both hubs.

Non-TTY gotchas in the same flow:
- `aida session end <id>` refuses without `--yes` ("confirmation is required and stdin
  is not a terminal"). It prints its Effects list first, then errors — the lease is
  still held, so re-run with `--yes`.
- The lease short-id in a handoff may be mistyped; `aida session leases` is the
  authority. `aida session end` matches on the lease id, and its error text
  misleadingly calls it a branch ("No lease found for branch `X`").

## Refinement observed 2026-09-29 (BUG-1729, twice, deterministic)

The auto-bump does **not** require `aida queue done` first. After merging #2285 and
again after #2286, `aida pull` moved BUG-1729 straight from **In Progress →
Completed** on the `(BUG-1729)` trailer alone:

```
auto-bumped 1 spec → Completed: BUG-1729 (was In Progress, commit a0e8a5c)
```

So the two-step above describes one path, not a precondition. **Any merged commit
whose trailer names the spec will complete it on the next `aida pull`, from
whatever status it was in.**

**The hazard this creates:** a spec with several acceptance criteria gets closed by
the first PR that merely closes *one* of them, because the trailer format cannot
express "partial". There is no way to suppress it from the commit side without
violating the required `[AI:tool] type(scope): summary (SPEC-ID)` format. So when a
PR closes only part of a spec:

1. Expect the bump. Say so in the PR body so the next reader is not surprised.
2. After `aida pull`, check the status and reopen:
   `aida edit <ID> --status in-progress --force`.
   Plain `aida edit --status` **refuses** with "Re-opening a closed requirement is
   usually a mistake — pass --force to override."
3. Then `aida remote reconcile --execute` to push the correction.

Do this before anything else after the pull — a spec left reading Completed will be
archived by the `next` hint (`aida archive <ID>`) with its criteria still open.

### reference_nvme_fast_scratch_migration

Since 2026-10-03 the host's write-hot agent state lives on `/mnt/fast` (22G Apple NVMe, ext4, `LABEL=fastnvme`, noatime, in fstab), moved off the single 1TB spinning HDD (`sda`, the only large disk — everything else funnels through it and it saturates easily; sda runs BFQ-candidate mq-deadline, see BUG-1789 session):

- `~/.cache/sccache` → `/mnt/fast/sccache` (symlink; cap stays 10GiB)
- `~/.codex/sessions` → `/mnt/fast/codex-sessions` (symlink)
- `~/.gemini/antigravity-cli` → `/mnt/fast/antigravity-cli` (symlink)

Maintenance: `~/.local/bin/fast-tidy.sh` via weekly systemd user timer `fast-tidy.timer`. It zstd-compresses codex rollouts older than 90 days (30 days when the partition is ≥82% full; steady-state baseline is 75%), and archives agy conversation `.db` files idle >60 days to `~/.gemini/antigravity-archive/conversations` on the HDD. Compressed `.jsonl.zst` files are invisible to codex resume and to AIDA's session scans ([[feedback_orphaned_load_generators_poison_every_measurement]] still applies when timing anything on sda).

Gotchas: moving any of these dirs requires the owning processes stopped (SQLite WAL mid-copy = corruption risk); the safe pattern used was atomic same-fs rename → symlink immediately → idle-priority rsync backfill → delete old. `/mnt/fast` must stay joe-owned at the root. Related: BUG-1789 (role enter's 1.4GB session scan) is mitigated but not fixed by the faster disk and the compression dropping old files from the scan set.

### reference_recordingforge_makes_orchestrator_pr_tests_hermetic

`RealPhaseDriver` has a `forge_factory: Option<ForgeFactory>` seam (TASK-1421).
Inject `crate::forge::fake::RecordingForge` (in `aida-cli-lib/src/forge.rs`,
`#[cfg(test)]`) and the driver's PR lookups answer from canned values:

```rust
let mut forge = crate::forge::fake::RecordingForge::new();
forge.open_for_branch   = ChangeLookup::Found(ChangeRef { id, url, branch, base, title });
forge.merged_for_branch = ChangeLookup::Found(/* … */);   // the BUG-709 arm
let mut d = driver(&root, "NFR-56", stub);
d.forge_factory = Some(forge.factory());
```

`forge.closed()` returns every `close_change` call, for asserting the
publication boundary.

**The `AlreadyMerged` arm needs a git repo with no `origin`.** `detect_phase1_pr`
only consults `merged_change_for_branch` when `probe_branch_on_origin` is *not*
`Absent`; with no remote, `git ls-remote` fails and the probe is
`LsRemoteFailed`, which reaches that arm. A fixture with no git repo at all
takes a different branch.

Used by `bug_1629_phase1_recovery_tests.rs`'s BUG-1769 tests. Related:
[[project_forge_plumbed_not_wired]].

### reference_run_test_binary_needs_rust_min_stack

`.cargo/config.toml:10` sets `RUST_MIN_STACK = "8388608"` in its `[env]` block. Running the
built test binary **directly** — the standing BUG-1729/BUG-1731 contention-reproducer recipe,
adopted to avoid cargo build slots — does NOT read that file, so it gets the 2 MiB default.

`agent_launcher_tests::agent_new_parses_prompt_file_and_rejects_conflicting_prompt_sources`
(added 2026-09-27, `7324a5b437`) then overflows its stack and **aborts the whole test process**
(exit 134). No `test result` line is printed at all, so a predicate grepping for failures sees
silence, which looks exactly like "still running" or "nothing wrong". Bracketed: fails at
`RUST_MIN_STACK=2097152`, passes at `8388608`. CI is unaffected because `cargo test` reads `[env]`.

**Always prefix `RUST_MIN_STACK=8388608`** when invoking a test binary directly, and anchor
suite predicates on `^test result` / `^failures:`, never on the absence of an error string.

Related: [[feedback_suite_log_predicates_must_anchor_on_harness_lines]],
[[feedback_never_conclude_from_truncated_command_output]].

### reference_run_user_tmpfs_is_only_7g

`/run/user/1000` is a **7.1 GB tmpfs**. `/run/user/1000/cargo-slots-1000/` is for advisory lock
files only (`slot-0.lock`, `slot-1.lock`, `queue.lock`). On 2026-09-27 it held `debug/` (6.8G) and
`aida-review-623ee41/` (280M) — Cargo target directories — at **100%, 0 bytes free**. A seat's build
died at link time with "Disk full?" and a bus error; it correctly refused to commit anything.

**Why it happens:** seats get `--add-dir /run/user/1000/cargo-slots-1000` so codex can `flock` the
slot files. That grants write access, and a seat can mistake it for a sanctioned target location.
See [[reference_codex_needs_add_dir_for_cargo_slots]] and [[BUG-1702]].

**How to apply:**
- `df -h /run/user/1000` belongs in any build-failure triage; a full tmpfs also threatens the
  systemd user session and dbus sockets, not just cargo.
- Before reclaiming, confirm no live build holds it: no `lsof +D` hits, nothing written in ~20min,
  no process with that path in `CARGO_TARGET_DIR`. Then delete the target dirs and **keep every
  `.lock` file** plus the `.agents/`, `.codex/`, `.git/` subdirs.
- Never set `CARGO_TARGET_DIR` under `/run/user`. Distinct from
  [[feedback_reclaim_disk_by_deleting_worktree_targets]], which is about `/home` at 97%.

### reference_suite_aggregate_line_is_the_largest_passed_count

`cargo test -p aida-cli-lib --lib` produced **51** `test result:` lines on 2026-09-30. Fifty of
them read `1 passed; 0 failed; 7421 filtered out` — nested-harness re-execs, where a test spawns
the test binary again with a filter. The real aggregate was the single line

```
test result: ok. 7420 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 584.55s
```

`head -2` on the log shows only re-exec lines, which reads as a filtered run and looks like
something went wrong. Surface the aggregate with:

```bash
grep -E '^test result:' "$LOG" | sort -t' ' -k4 -rn | head -1
```

The verdict that actually matters is still the pair the discipline already names: the process exit
code, and `grep -oP '^test result:.*?\K[0-9]+(?= failed)' | sort -u` yielding only `0`.

Also expect `error:` lines in a green log — this crate's fixtures deliberately provoke git
failures (`failed to push some refs`, `untracked working tree files would be overwritten`) inside
their own tempdirs. Four of them appeared in the green run above.

Related: [[feedback_suite_log_predicates_must_anchor_on_harness_lines]],
[[feedback_never_conclude_from_truncated_command_output]],
[[feedback_source_scanning_guards_need_the_full_suite]].

