# CLAUDE.md

Guidance for Claude Code working in the AIDA repository. Project background and high-level architecture live in `OVERVIEW.md`; this file focuses on conventions and reference material you need while writing code here.

## Project orientation

AIDA = AI Design Assistant. The defensible niche is the **agent-collaboration layer**: stable spec IDs, typed relationships, code-to-spec trace comments, and an MCP server that exposes the requirement graph to coding agents. Karpathy-style "structured markdown queryable by Claude" is the floor; AIDA adds the relationship graph + identifier stability + enforcement loop.

**Strategic positioning** (Trojan-horse, 2026-05-14): the visible product is intentionally simple — a TUI wrapping Claude Code sessions (EPIC-26). The *"so what? I could do this in 20 lines of bash"* reaction on first sight is the *intended* impression. The actual value — graph, IDs, traces, MCP, queue, lifecycle — surfaces through use, not the surface. When adding features or polish, the test is **"does this make the TUI's quiet depth stronger when someone digs in?"** Surface complexity in the TUI itself is anti-pattern. See `OVERVIEW.md` "Public face: the TUI is the product" section for the full framing.

For the full vision, architecture, and surface inventory see `OVERVIEW.md`. For the path-forward audit and current direction see `docs/plans/2026-05-02-git-canonical-storage.md`. For the **autonomy + escalation + inter-agent comms architecture** — the three-mode autonomy ladder, the implementer → advisor → human escalation cascade, the advisor's Type A/B/C calibration, and the file-based handshake substrate — see `docs/architecture/autonomy-and-escalation.md` (paired with `docs/autonomous-drain.md` for the practical user guide). For wider market landscape comparisons and refresh signals see `docs/competitive-analysis/`. For the *"should I use AIDA or X?"* question in any of its forms see `docs/positioning/` (one focused comparison per neighbor tool; lead with the two nearest competitors `vs-spec-kit.md` + `vs-kiro.md`, then `vs-ultrareview.md`, `vs-ultraplan.md`, `vs-claude-code-subagents.md`, `vs-karpathy-md.md`, `vs-saas-pm.md`). The current competitive picture — the moat, the commoditized-vs-differentiated split, the capability roadmap, and the tripwires — is `docs/competitive-analysis/2026-05-31-round2-moat-gaps-moves.md`. For precise definitions of AIDA's machinery vocabulary — orchestrator, phase, drain, lease, role, scope, session, worktree, sentinel, batch, autonomy mode — see `aida-core/templates/docs/aida/discipline/machinery-glossary.md` (the scaffolded discipline-pack glossary; companions: `lifecycle-vocabulary.md` for spec-state verbs). For the TUI — keybindings, status overlay, autonomous drains, crash recovery — see `docs/tui/README.md` (`aida tui`, shipped default-on since STORY-137).

**Workspace** (6 crates): `aida-core` (engine), `aida-cli` (`aida` binary + MCP server), `aida-crate` (published `aida` crate metadata), `aida-server` (REST + gRPC, port 8080), `aida-generate-types` (Rust → TypeScript), `aida-tui` (the `aida tui` terminal shell, EPIC-26). React dashboard at `aida-web-react/` (port 5173 dev). Native desktop and WASM clients were extracted to a separate repo on 2026-05-02.

## Storage model (EPIC-1-001)

**Git-canonical by default.** The orphan `aida-store` branch is the writer of record (one YAML file per requirement under `objects/TYPE/000/SPEC-ID.yaml`). A SQLite cache at `.aida/cache.db` (gitignored, auto-rebuilt) is a rebuildable read projection used to make list/filter/search fast without scanning hundreds of YAML files. Writes go to git first, then the cache (write-through). Stale-detection compares the cache's recorded HEAD SHA against the orphan branch's actual HEAD; mismatch triggers rebuild on next read.

- Live worktree: `.aida-store/` (gitignored)
- Branch: `aida-store` on origin
- Cache: `.aida/cache.db` (gitignored)
- Manage cache: `aida cache status`, `aida cache rebuild`

**Per-spec transition history lives INSIDE the YAML.** Every `objects/TYPE/000/SPEC-ID.yaml` carries a `history:` array of `HistoryEntry` records, each with `id` (UUID), `author`, `timestamp`, and a `changes:` list of `{field_name, old_value, new_value}` triples. Every status flip, priority change, tag edit, owner change, etc. lands here as a structured row. **This is the source-of-truth for spec-state time series** — `aida history --events` and `--spec <id>` read from it; reviewers building burn-down charts or status-flow analyses should walk these arrays directly rather than approximating from `modified_at`. The cache (`.aida/cache.db`) is a derived read-projection and does NOT currently expose history rows; for substrate-grounded time series, read the YAML or the orphan-branch git log. trace:TASK-121 | ai:claude

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
aida init --with-memories      # Also write the starter memory pack (opt-in)
aida init --with-memories --refresh   # Overlay updated pack files, keep your edits
aida init --force              # Overwrite existing files
```

`aida init` creates: orphan branch `aida-store` + worktree at `.aida-store/`, `.aida/config.toml`, `.aida/cache.db`, META requirements seeded into the orphan store, `.mcp.json`, `CLAUDE.md`, `AGENTS.md` (Codex), `.claude/skills/` + `commands/` + `hooks/`, `docs/plans/`, `docs/aida/discipline/`.

### First-user demo — `scripts/aida-demo.sh` (TASK-563)

To validate that `aida` is operational end-to-end on a fresh project — without polluting your real workspace — run the bundled demo script:

```bash
bash scripts/aida-demo.sh              # interactive walkthrough (Enter-to-continue between sections)
bash scripts/aida-demo.sh --auto-cleanup  # skip cleanup prompt (for CI / scripted runs)
```

The script creates a throwaway public GitHub repo (timestamped name like `aida-demo-20260525-...`), clones it locally, runs `aida init`, walks through filing a spec + implementing + committing with the `(SPEC-ID)` trailer convention + `aida pull` auto-bump, then prompts for cleanup (defaults to keep so you can poke around). Useful for: first-user evaluation, demo recordings, sanity-checking a fresh `aida` build initializes cleanly.

Prerequisites: `aida` on PATH (run `aida-on` first if using the dev build), `gh` CLI authenticated, `git` configured with `user.name` + `user.email`.

### Starter discipline pack (STORY-255)

`aida init` ships AIDA-using *discipline* — the habits and vocabulary that make an AIDA project run well — as scaffolding, so a new project inherits it instead of re-discovering the same friction. Three channels:

- **`docs/aida/discipline/`** (always) — nine canonical guides (`README.md`, `advisor-role.md`, `lifecycle-vocabulary.md`, `machinery-glossary.md`, `workflow-patterns.md`, `session-discipline.md`, `skill-prompt-kinds.md`, `substrate-as-bouncer.md`, `robust-project-root-resolution.md`). Master templates: `aida-core/templates/docs/aida/discipline/`, embedded via `build.rs`, scaffolded by `ensure_discipline_pack_scaffold` (idempotent — `--force` to overwrite). `advisor-role.md` documents the **advisor** seat. `advisor` is the canonical role identifier everywhere — config, env vars (`AIDA_SESSION_ROLE`), queue routing, statusline, role files. `dialog` (the old internal token from TASK-279) is now a deprecated, silently-accepted alias for it, normalized to `advisor` at every role-name boundary so a not-yet-migrated machine's `dialog.toml`/config/shells keep working. trace:TASK-586 (supersedes TASK-279)
- **CLAUDE.md discipline section** (always) — `generate_claude_md` appends a "Discipline for AIDA-using sessions" section pointing at the pack.
- **Starter memory pack** (`--with-memories`, opt-in) — the generic discipline memories under `aida-core/templates/memories/` written to `~/.claude/projects/<slug>/memory/`. The pack is **marker-driven**: every memory file carrying `propagation: scaffolding-pack` in frontmatter ships, so the set grows just by tagging new generic memories. Scaffolded files get `originSessionId: aida-scaffold` + a `scaffoldChecksum` (FNV-1a of the body). `aida init --with-memories --refresh` overlays newer versions of files the user has *not* edited (body checksum still matches) and leaves edited or unmarked files alone. `MEMORY.md`'s `<!-- aida:scaffold-pack -->` block is regenerated; user content outside the markers is preserved.

When adding a new generic discipline memory, tag it `propagation: scaffolding-pack` and it joins the pack on the next build — no code change.

### Daily-use commands

```bash
aida list                              # Cache-backed (sub-ms vs full-store load); default excludes archived
aida list --status draft               # Filter by status
aida list --archived                   # Only archived rows; --all = both (STORY-441)
aida search "<query>"                  # Cache-backed FTS5 search (same archive filter as list)
aida history                           # Recent activity, incl. freshly-Completed; archive hides long-tail (STORY-441)
aida archive <ID>                      # Mark a spec archived (hidden from default views, audit trail preserved)
aida archive --older-than 30d --dry-run   # Preview bulk sweep; drop --dry-run to apply
aida unarchive <ID>                    # Restore an archived spec
aida show <ID>                         # Show requirement details + git linkage (commits/files/branch/PR — TASK-241)
aida show <ID> --no-git                # Skip the git-linkage section; --verbose expands it
aida add --title "..." --type story --status draft --tags "tag1,tag2"
aida edit <ID> --status completed
aida comment add <ID> "..."
aida db merge-gate                     # Assign agreed short IDs (FR-7-001 → FR-1)
aida db sync --pull --push             # Sync orphan branch with remote
aida fetch                             # Read-only two-leg refresh of remote refs (TASK-107)
aida fetch --code-only --quiet         # Background-safe code-leg-only refresh
aida db reconcile-status               # Replay Done→Completed bumps the pull missed (TASK-226)
aida db reconcile-status --spec STORY-86  # Targeted replay for a single spec
aida db reconcile-status --since v0.5.1 --dry-run  # Preview a bounded replay
aida cache status                      # Compare cache HEAD vs git HEAD
aida plan verify docs/plans/<file>.md  # Lint a plan: drifted refs, missing files/sections (TASK-93)
aida plan verify <file> --fix          # Rewrite drifted path:line refs in place
aida plan helpers <spec>               # Derive a 'Reusable helpers' section from the trace graph (TASK-94)
aida plan helpers <spec> --append <file>  # Append the derived section to a plan file
aida ultraplan <spec>                  # Assemble a rich /ultraplan prompt from spec context; copy to clipboard (TASK-113)
aida ultraplan <spec> --stdout         # Print the assembled prompt instead of copying
aida ultraplan <spec> --json           # Emit prompt + warnings + token estimate as JSON
aida goal --batch <name>               # Derive a machine-checkable /goal completion condition from AIDA metadata (TASK-242)
aida goal --epic <ID> --pr <N>         # Flags (--batch/--epic/--spec/--pr/--queue-empty) compose with AND
aida goal --spec <ID> --copy           # --copy → clipboard; --invoke → bare `/goal …` line for scripting
aida changelog generate                # Print structured CHANGELOG.md to stdout — every release + [Unreleased] (TASK-299)
aida changelog refresh                 # Rewrite CHANGELOG.md (idempotent — same git state → byte-identical output)
aida changelog preview                 # Stdout-only preview of the [Unreleased] section
aida brief <agent> TASK-492 --note "why this, why now"  # Write a local pickup brief under .aida/agent-briefs/
aida brief list --for-agent <agent>      # List pending briefs for a target agent; add --include-acked for audit
aida brief ack .aida/agent-briefs/<agent>/<file>.md  # Mark a brief acknowledged
aida agent new claude --role implementer --spec <SPEC-ID>  # Supervised launcher with registry + role context
aida agent new claude --role advisor --show-context        # Print the generated launch-context snapshot
aida --asciinema queue work --batch overnight-X --auto-complete  # Record a demo/training/audit cast under project-local .aida/casts/ (falls back to ~/.aida/casts/)
```

`aida brief <agent> <SPEC-ID> [--note <STR>|--note -]` turns the
operator's pickup message into a substrate-resident markdown file at
`.aida/agent-briefs/<agent>/`. The directory is local runtime state under
the existing `.aida/*` deny-by-default gitignore block. Use `brief list`
to route work without scrollback and `brief ack` after the agent has read
the file. MCP-speaking agents should use the equivalent brief tools:
`list_briefs`, `read_brief`, and `ack_brief`.

`aida agent new <type>` is the supervised launch path for Claude, Codex,
and Antigravity. It registers the child process under `.aida/agents/`,
writes a point-in-time role-context snapshot under
`.aida/agents/context/`, and passes that path as
`AIDA_AGENT_CONTEXT_FILE`. The snapshot includes role guidance, the active
lease/spec, pending brief paths with titles, and queue-head hints. Use
`--show-context` to print it before spawn, or `--no-context` for a bare
launch. The file is a startup snapshot only; keep polling briefs/MCP for
work filed after launch.

For non-Claude MCP client setup and marketplace/distribution surfaces, keep
`docs/agents/aida-mcp-install-matrix.md` current. It is the operational matrix
for connecting AIDA to Claude Code, Codex, Cursor, Windsurf, Continue, Cline,
Copilot, Devin, Sourcegraph/Amp, and adjacent agent clients.
Before publishing AIDA through a marketplace or registry, run
`docs/security/marketplace-publication-checklist.md`.

`aida --asciinema [--cast-out PATH] [--cast-title STR] <subcommand>` is
the first-class capture wrapper for demos, training corpus material, and
autonomous-drain audit trails. It no-ops gracefully when `asciinema` is
missing or the invocation is not attached to a TTY. By default, casts are written to `.aida/casts/` at the project root, falling back to `~/.aida/casts/` if not run inside a project directory (defined by the presence of `.git` or `.aida/`).

`aida queue list` (TASK-222) appends a **Done — awaiting merge** section below the queued items so freshly-shipped work stays visible until the auto-bump fires. Pass `--no-in-flight` for the queued-only view, or `--in-flight-only` to focus on "what am I waiting on a PR for."

**Tag conventions** (TASK-512): subcommand-identifying tags use the `aida:<subcommand>[:<verb>][:<sub-verb>]` colon-namespaced form (`aida:status`, `aida:queue:work`, `aida:db:sync:pull`) so `aida list --tags 'aida:queue:*'` returns every spec touching that surface. Behavior / pattern / provenance / severity tags stay flat (`orchestrator`, `papercut`, `from-self-test`, `ceiling-pattern`). Existing colon namespaces continue unchanged — `batch:NAME`, `lifecycle:trivial`, `severity:cosmetic`, `parent:EPIC-31`, `depends-on:phase-1`, `subsumes:TASK-N`, `from-review:PR-N`, `kind:bug-spotted`. Multi-touch specs get multiple subcommand tags. TASK-511 swept the historic flat forms (`aida-*`, `queue-*`, `session-*`) onto this namespace — re-runnable via `scripts/migrate-tag-namespace.sh` when new flat hyphen-form tags slip in. Full rules + anti-patterns: `docs/aida/discipline/tag-conventions.md`. trace:TASK-512 trace:TASK-511

**Batch tag convention** (TASK-229): items sharing a `batch:NAME` tag (set via `aida edit <id> --tags batch:NAME`) compose with two commands. `aida queue list --batch NAME` filters both the queued and in-flight sections to that batch. `aida queue work --batch NAME` picks the head queued member of that batch (head-pickup loop — re-run after each session exits to drain the next one); `--dry-run --batch NAME` lists the pickup order without acting. `aida queue progress --batch NAME` (TASK-232) shows the bucketed view of the batch's lifecycle (Shipped / In flight / Working now / Remaining). `aida queue work --batch NAME --auto-complete` (TASK-285) drains the whole batch autonomously — one full implementer→CI→reviewer→merge→pull→build lifecycle per member, advancing the head after each — until the batch is empty, `--max N` is reached, or a phase fails un-shelvably. The `--auto-complete=through-ci` / `through-merge` variants compose too. **EPIC-28 resilient drain**: a *shelvable* phase failure (CI red, reviewer RequestChanges, build failed, …) parks the spec in `NeedsAttention` with a structured `FailureReason` and the drain *continues*; dependent members (`BlockedBy → <shelved>`) skip automatically via the existing pickability gate. The drain exits **`2`** (not `0`/`1`/`3..=8`) when at least one member shelved or skipped, so scripts know to triage. Cap shelving with `--max-failures N` (default `5`); pass `--max-failures 0` for the historical "first failure stops" behaviour. Triage shelved + skipped with `aida findings list`. Full details: `docs/autonomous-drain.md` → "Shelving on failure".

**Lifecycle short-circuit tags** (STORY-442): specs tagged `lifecycle:no-ci-wait`, `lifecycle:no-review`, or `lifecycle:no-build` skip only that non-integrity phase during `aida queue work --auto-complete`; `lifecycle:trivial` is shorthand for all three. CI still runs remotely when `no-ci-wait` is set; the orchestrator just does not block on it. Merge and pull/auto-bump never skip, so completed-state hygiene is preserved. Use these tags only for low-risk, small-blast-radius work where lower latency is worth less redundancy.

**Calibration mode** (STORY-347): with `[advisor] calibration_mode = "on"` in `.aida/config.toml` (or `--calibrate` per-drain) every advisor-tier punt produces **two** verdicts side-by-side — cold-boot (drives the drain) plus fork-from-live (shadow only). Recorded to `.aida/punts/<punt-id>/calibration.yaml`; review with `aida findings calibration` (default shows disagreements — the substrate-gap signal), `--stats` for the rolling agreement rate, `aida findings calibration annotate <punt-id> "gap → wrote memory <name>"` to record the closing memory. Cost is real (both runs fire); turn it on to mine substrate gaps, off when you trust the substrate. Full guidance in `docs/autonomous-drain.md`.

**Headless drain** (`--no-human`, STORY-263 / STORY-276): `aida queue work --auto-complete --no-human` launches orchestrator phases headless (`claude -p`) so they advance without a Ctrl+D — the basis for an unattended overnight drain. Two modes: bare `--no-human` / `--no-human=reviewer-only` runs the reviewer (phase 3) headless and leaves the implementer (phase 1) interactive; `--no-human=both` runs phase 1 headless too (STORY-276). The headless implementer's safety net is the **punt** — on a design-fork it cannot safely resolve it runs `/aida-punt` instead of guessing, parking the spec in NeedsAttention; the orchestrator detects the punt (via the `AIDA_PUNT_SIGNAL_FILE` handshake) and routes it through the **advisor tier** (STORY-306): it spawns a headless advisor (`/aida-advise`) that either resolves the fork — the implementer session resumes (`claude -p --resume`) with the judged answer and the drain continues — or escalates it to a human. `--escalate-blocks` (the default) parks an escalated spec for morning triage; `--escalate-defaults` resumes the implementer to ship the defensible default instead. The advisor's bias is conservative: it resolves only a fork grounded in a recorded principle/preference and escalates everything else. `aida findings list` shows what the overnight advisor resolved vs escalated. The escalation handshake also covers the reviewer — a headless reviewer that won't auto-merge writes `merge: escalated-to-human` in its verdict file, and the orchestrator treats that as a first-class non-failure outcome. `--unattended` / `--headless` are aliases. Kickoff prints a per-mode scope banner requiring a one-time ack (skip with `AIDA_NO_HUMAN_ACKNOWLEDGED=1`), and an orchestrated interactive phase shows `auto:N/6` + `pause-here` in the statusline (TASK-306). The trade-off is real — **interactive = better quality decisions, autonomous = better throughput** — so pick per session: drive known design-fork specs at the keyboard, drain mechanical batches headless. Headless launches force `--permission-mode bypassPermissions` and stream their JSON to `.aida/headless-logs/`. Full guidance + the SPIKE-7 evidence behind every flag: `docs/autonomous-drain.md`.

### Queue identity (BUG-89)

The queue's `user_id` is the **shell's** user identity — not the node identity from `~/.aida/node.toml`, not the email in `[node]`, not the role's stored `user_id`. Every queue path (`add`, `list`, `next`, `done`, `remove`, `move`, the role-show queue head, the statusline depth) routes through `current_user_id()` in `aida-cli`, which resolves in order: `--user <id>` flag → `AIDA_USER` env → `USER` env → `USERNAME` env (Windows) → `"default"`. If `aida queue list` ever returns nothing where you expect items, check `echo $USER` and `echo $AIDA_USER` first — the queue is keyed off whichever the shell sees.

### Proactive requirements workflow

**Requirement-first development.** Before implementing any feature or fix, ensure a requirement exists:

1. Check if work has a SPEC-ID. If not: `aida add --title "..." --description "..." --status approved`
2. During coding, add trace comments: `// trace:FR-1-042 | ai:claude`
3. Before committing: use `/aida-commit` to ensure all changes are linked

If you work conversationally without explicit `/aida-req` calls, use `/aida-capture` at session end to review and capture any requirements that were discussed but not yet added.

### Plan archival

Every implementation plan must be saved to `docs/plans/YYYY-MM-DD-<slug>.md`. Use `docs/plans/_TEMPLATE.md` (scaffolded by `aida init` from `aida-core/templates/plan-template.md`) as the starting structure — 11 sections cover Approach + diagram, Decisions, Files (in build-order), Critical Files, Reusable helpers, Risks + gotchas, Tests (named), Verification (executable), Followups, and Related. The header carries Date / Specs / Status / Complexity. trace:TASK-92

**Symbol refs over line refs.** When citing code from a plan, prefer symbol refs (`fn handle_pull_command`, `struct ImplementationInfo`) over line refs (`main.rs:19713`). Symbol refs survive edits; line refs drift fast and are often stale within hours of generation. Worked example: `docs/plans/2026-05-13-story-86-done-status.md`.

**Verify before relying on a plan.** `aida plan verify docs/plans/<file>.md` (TASK-93) lints a plan against the template: it reports drifted `path:line` refs (with the corrected line, located by symbol name), missing files, and absent required sections (Critical Files, Verification, Followups are hard requirements). It exits non-zero on any missing file or section, so it works as a pre-commit hook on `docs/plans/`. `--fix` rewrites drifted refs in place; `--quiet` drops the per-check OK lines. Refs inside `<!-- -->` comments and fenced code blocks are skipped. trace:TASK-93

**Followups get filed, not forgotten.** When a spec reaches Done (`aida queue done`) or Completed (the STORY-86 auto-bump on merge), AIDA parses the `## Followups` section of any plan that owns that spec and offers to file each bullet as a child TASK. `aida queue done` prompts per bullet (`[y/N/skip]`); `--yes` and the non-interactive auto-bump path file all. Idempotent — a `[aida:followups]` marker comment on the spec records what was filed and declined, so whichever path runs first wins and declines are never re-filed. Opt out with `AIDA_AUTO_FOLLOWUPS=false`. trace:TASK-96

**The plan rides into the session.** `aida queue work <spec>` discovers any plan that owns the spec and pre-populates the session manifest with a *plan brief* — the `## Critical Files`, `## Followups`, and `## Verification` sections. `aida session show --plan` renders it, and `/aida-pickup` leads its first message with it so the implementer gets the blast radius and definition of done without grepping for the plan. Graceful no-op when no plan file exists. trace:TASK-95

**Reusable helpers come from the trace graph.** `aida plan helpers <spec>` derives a `## Reusable helpers` section by walking the requirement graph — sibling specs (same parent), tag-mates, and (when discriminating) same-feature specs — and harvesting their `// trace:` comments for the files + symbols they already touch. It ranks siblings and tag-mates above the coarse same-feature set and only surfaces specs that name a helper, so the output stays a focused "don't reimplement this" brief. `--append <plan-file>` writes the section straight into a plan. trace:TASK-94

**Hand `/ultraplan` a fully-contextualised prompt.** `aida ultraplan <spec>` assembles a structured planning prompt — the spec's description, extracted `## Acceptance` criteria, parent/child/sibling context, the AIDA 11-section plan structure, and the trace-graph reusable helpers — and copies it to the clipboard (`--stdout` / `--json` for piping). It turns a terse ask into a brief `/ultraplan`'s explorers can anchor on, and the inlined plan structure means the returned plan already matches `docs/plans/_TEMPLATE.md`. trace:TASK-113

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

`aida dev activate` prepends `target/{release,debug}/` to PATH — prefers the binary whose embedded git SHA matches (or is an ancestor of) the current branch HEAD (TASK-221), so switching branches between builds doesn't silently leave you on a binary built from the other branch's source. Falls back to most-recently-built with a `Warning:` when neither binary matches the current HEAD. `aida dev status` shows the active binary's SHA, current HEAD, and the match verdict (`exact match` / `ancestor of HEAD` / `DIVERGED from HEAD`). Prefixes the shell prompt with `(aida-debug)` or `(aida-release)` so the active build is visible at a glance. `aida dev deactivate` undoes both.

For releases, `scripts/release.sh {major|minor|patch|<explicit>}` bumps the workspace version, regenerates `CHANGELOG.md` via `aida changelog refresh --released-as v<new>` so the changelog commits *with* the version bump (TASK-299), generates tag notes from `git log <prev>..HEAD`, commits, tags, and pushes (which triggers `.github/workflows/release.yml` to build and publish binary tarballs).

**CI is split for alpha cycle time** (TASK-257). PR CI (`.github/workflows/ci.yml`) is **Linux-only** — ~3-5 min cycle. Windows + macOS are validated by `.github/workflows/cross-platform.yml`, which runs on a nightly cron (06:00 UTC) and on manual `workflow_dispatch`. Check the latest nightly results before relying on cross-platform behaviour: <https://github.com/joemooney/aida/actions/workflows/cross-platform.yml>. **Releases require cross-platform CI green within 24h of tagging** — `scripts/release.sh` calls `scripts/pre-release-check.sh` before the tag step, which reuses a `<24h` green run or dispatches a fresh `gh workflow run cross-platform.yml` and blocks on it. Opt out with `--skip-xplat-check` / `AIDA_SKIP_XPLAT_CHECK=1` (not recommended for a published release). Re-add Windows + macOS to PR CI once there are non-Linux users and the cross-platform matrix has been quiet for 2+ weeks.

**Cross-worktree cargo cache gotcha** (TASK-0396): cargo's `target/.fingerprint/` references absolute paths from the build that produced each artifact. If `aida session end` removes a worktree, subsequent `cargo build` from a sibling worktree can fail with errors pointing at the deleted worktree's paths. Recovery: `cargo clean -p <crate>` for the affected workspace members, or `cargo clean` for a full reset. See `docs/session-lifecycle.md` for the full recipe.

**Usage telemetry** (STORY-122): every `aida` invocation appends a single JSONL line at `~/.aida/usage.jsonl` with the command shape (e.g. `queue list`), `args_count`, `exit_code`, and `duration_ms`. Privacy floor: no argument values, no file paths, no requirement content. Opt out with `AIDA_TELEMETRY=0` or `[telemetry] enabled = false` in `.aida/config.toml`. Query with `aida usage` (top-20 in last 30d), `aida usage --unused 30d` (deprecation candidates), `aida usage --errors` (high error-rate commands), or `aida usage --json` for machine consumers. The monthly-cadence synthesis surface is `/aida-insights` (TASK-577) — wraps `aida usage` + `aida usage --auto-complete` + `aida findings calibration --stats` into the three top-line signals (most-used, drain success, calibration agreement) and the deprecation / UX-gap / orchestrator-fix / substrate-gap follow-ups they suggest. The log is local-only and never phoned home.

### Divergent-branch recovery

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

`aida init` scaffolds 40 skills under `.claude/skills/` and matching slash commands under `.claude/commands/`. Daily drivers: `/aida-req`, `/aida-implement`, `/aida-commit`, `/aida-capture`, `/aida-doc`, `/aida-search`, `/aida-plan`, `/aida-rebase`, `/aida-onboard`, `/aida-drain-queue`. The `/ultraplan` round-trip pair: `aida ultraplan <SPEC>` assembles the prompt, `/aida-import-plan <FILE>` lands the saved output back under `docs/plans/` (TASK-113/TASK-114). The advisor's narrative report: `/aida-digest [--since <window>] [--audience customer|team|self]` (STORY-252). The advisor's monthly telemetry-pattern review: `/aida-insights` (TASK-577). Orchestrator-internal: `/aida-advise` is the headless advisor tier (STORY-306) — spawned by `--auto-complete --no-human=both` on a punt, not run by hand. Run `aida` (no args) for the full CLI, or `ls .claude/skills/` for the full skill catalog.

### MCP server

`aida mcp-serve` exposes requirements as MCP tools and resources for native Claude Code integration via `.mcp.json`. Tools: `list_requirements`, `show_requirement`, `add_requirement`, `update_requirement`, `search_requirements`, `add_comment`, `add_relationship`, `list_features`, `history`. Resources: `aida://project/summary`, `aida://requirements/tree`. The MCP server is the highest-leverage surface for the agent-context vision.

Long-running MCP servers self-respawn after handled requests when the on-disk `aida --version` reports a newer package version or a different build SHA for the same version. The current MCP response is flushed first; the next request runs on the new binary. If a client still appears stale, kill that agent's `aida mcp-serve` process and let the MCP client respawn it.

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

- `--type` (lowercase): `functional`, `non-functional`, `system`, `user`, `bug`, `epic`, `story`, `task`, `spike`, `sprint`, `folder`, `meta`, `doc`
- `--feature`: feature category name (NOT a type)
- `--status`: `draft`, `approved`, `planned`, `in-progress`, `done`, `completed`, `rejected`
  - The full state machine — Draft → Approved → Planned → In Progress → Done → Completed → Released, with the precise verb for each transition and the edge cases (cluster PRs, parallel pipelining, autonomous drains) — is documented in `docs/lifecycle.md` and the README's "Spec lifecycle" section. trace:TASK-273
  - **`done` vs `completed` (STORY-86)**: `done` means "work finished on a branch" (set by `aida queue done`). `completed` means "merged to the default branch." `aida pull` and `aida db sync --pull` auto-bump `done → completed` when a commit referencing the spec lands on main, so you typically don't set `--status completed` manually — let the merge promote it. **When the auto-bump misses** (BUG-96 made the YAML unreadable at pull time, or the spec flipped to Done after the referencing commit was already on local main), recover with `aida db reconcile-status` — a manual replay of the same scan over a wider window. Add `--spec SPEC-ID` for a targeted replay, `--since REF` to bound the range, `--dry-run` to preview without writing. trace:TASK-226
  - **archive ≠ status (STORY-441)**: `archived` is a view-level flag orthogonal to `status`. `aida list` / `aida history` / `aida search` hide archived rows by default; `--archived` shows only archived; `--all` shows both. A freshly-Completed spec is *not* archived — it stays visible in the default view until an explicit `aida archive <ID>`, a bulk `aida archive --older-than 30d --dry-run` (default csv: completed,rejected), or the opt-in auto-sweep on `aida pull` (gated on `[archive] auto_after_days = N` in `.aida/config.toml`, clamped to ≥7 days; opt-out with `AIDA_AUTO_ARCHIVE=0`). Archive ≠ deletion: the YAML, the audit trail, and the requirement graph all survive. trace:STORY-441
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
