---
marp: true
title: "AIDA — Administrator Guide"
description: "Standing up, configuring, and maintaining an AIDA deployment."
paginate: true
theme: default
---

<!--
RENDER:
  npx @marp-team/marp-cli@latest docs/presentation/aida-administrator-guide.md -o aida-administrator-guide.html
  npx @marp-team/marp-cli@latest docs/presentation/aida-administrator-guide.md --pdf -o aida-administrator-guide.pdf
AUDIENCE: whoever sets up / operates AIDA for a team. GOAL: install, configure, keep it healthy, ship it safely.
COMPANION DECKS: executive-briefing, developer-deep-dive, user-walkthrough.
Verify every command against `aida <cmd> --help` before presenting.
-->

# AIDA — Administrator Guide

### Stand it up, configure it, keep it healthy

Init · the shared/local split · config · maintenance · multi-node · security

<small>v0.11.0 · administrator guide</small>

---

## What an administrator owns

- **Initialization** — `aida init`, the storage mode, the scaffolded agent config.
- **The shared substrate** — the orphan `aida-store` branch on origin.
- **Per-machine identity** — node ids, queue user ids.
- **Health** — cache freshness, store sync, lease hygiene, `aida doctor`.
- **Policy** — `.aida/config.toml` (autonomy, archiving, telemetry, drain tuning).
- **Safe distribution** — the marketplace-publication security checklist.

> The store is **git**. Most of "administration" is keeping a branch synced and a cache fresh.

---

## Initialization

```bash
aida init                  # DEFAULT: distributed git-canonical (recommended)
aida init --sibling        # store in a sibling repo (multi-repo workspaces)
aida init --centralized    # legacy single-file SQLite (deprecated — warns)
aida init --no-skills      # skip .claude/skills/ + commands/
aida init --no-hooks       # skip git hooks + .claude/hooks/
aida init --with-memories  # also scaffold the starter discipline memory pack
aida init --force          # overwrite existing files
```

**Creates:** orphan branch `aida-store` + `.aida-store/` worktree · `.aida/config.toml` ·
`.aida/cache.db` · seeded META specs · `.mcp.json` · `CLAUDE.md` · `AGENTS.md` ·
`.claude/{skills,commands,hooks}/` · `docs/plans/` · `docs/aida/discipline/`.

<!--
The discipline pack (docs/aida/discipline/) ships the habits + vocabulary that make an AIDA project run well. --with-memories is opt-in; --refresh overlays newer pack files while preserving user edits.
-->

---

## Fresh clone: it just works (for reads)

A teammate clones the repo — **no manual store setup**:

```bash
git clone <repo> && cd <repo>
aida list          # auto-attaches .aida-store/ from origin + rebuilds the cache
```

- The first store-reading command **auto-attaches** the worktree from the `aida-store` branch (TASK-621).
- **Writing** new spec IDs needs a node id → `aida init` (full bootstrap) or `aida node acquire`.
- Re-running `aida init` on an initialized repo **reconciles the node id** rather than dead-ending (TASK-623).
- If distributed mode is declared but the store can't attach (offline), reads **error with guidance** — never a silent fall back to legacy YAML.

---

## What's shared vs per-clone-local

```
SHARED  (orphan `aida-store` branch on origin)      PER-CLONE  (gitignored .aida/)
─────────────────────────────────────────────      ──────────────────────────────
• every spec's YAML + history                       • cache.db        (rebuildable)
• typed relationships                               • config.toml     (local policy)
• node + ID-block registries                        • session leases  (worktrees)
                                                    • drain-state.json
                                                    • punts ledger, usage.jsonl
```

- `.gitignore` is **deny-by-default**: `.aida/*` + an explicit `!.aida/config.toml` allow-list.
- A new tracked file under `.aida/` needs its own `!.aida/<name>` line (BUG-73).

---

## Multi-node + the queue-identity gotcha

The **queue** is keyed off the **shell's user id**, not the node id. Resolution order:

```
--user <id>   →   $AIDA_USER   →   $USER   →   $USERNAME (Windows)   →   "default"
```

> If `aida queue list` is empty where you expect items, check `echo $USER` and `echo $AIDA_USER` first.

- Same person on two machines with different `$USER` (`joe` vs `joseph`) reads **two different queues**.
- **Fix:** pin `export AIDA_USER=joe` in the shell rc on every machine.
- Node ids (`~/.aida/node.toml`) are about **ID minting**, not queue routing — don't conflate them.

---

## Config: `.aida/config.toml` sections

| Section | Key knobs |
|---|---|
| `[deployment]` | `mode` (distributed/centralized), `store_path`, `store_type`, `branch` |
| `[id_format]` | `policy`: node-aware-only / blocks-then-fallback (default) / blocks-only |
| `[behavior]` | `permission_mode` for user-facing file writes |
| `[orchestrator]` | `auto_release_dormant_leases`, `stale_lease_threshold_minutes` |
| `[advisor]` | `calibration_mode` (off/on), fork settings for the headless advisor tier |
| `[archive]` | `auto_after_days` — opt-in auto-archive sweep on pull |
| `[telemetry]` | `enabled` — local usage log on/off |
| `[drain]` | `gh_verify_retries`, `no_progress_minutes`, `phase_ceiling_minutes` |
| `[store.sync]` | `auto_push`: manual / session-end / per-write |

<!--
[drain] is the newest section (drain-reliability tuning). [store.sync] auto_push="session-end" is the recommended multi-node default. Verify exact keys with the parsers in aida-cli/src before quoting in a real deployment.
-->

---

## Keeping the store synced

```bash
aida db sync --pull --push     # sync the orphan store with origin
aida fetch                     # read-only two-leg refresh (code + store refs)
aida fetch --code-only --quiet # background-safe
aida pull                      # code leg (ff-only) + store leg + Done→Completed auto-bump
```

Recommended multi-node rhythm:

```toml
[store.sync]
auto_push = "session-end"   # push the store when a session ends
```

- Code leg is **`--ff-only`** by design (won't surprise your tree); on divergence it prints the rebase hint.
- Store leg uses `--rebase` (conflicts are rare; the worktree is AIDA-managed).

---

## Cache + status recovery

```bash
aida cache status      # cache HEAD SHA vs orphan HEAD — fresh or stale?
aida cache rebuild     # force a full reproject from git (always safe)
aida db status         # store changes / sync state / conflicts
aida db check --collisions          # two specs claiming one short id
aida db reconcile-status [--spec ID] [--since REF] [--dry-run]
```

- The cache is **disposable** — when in doubt, `aida cache rebuild`.
- `reconcile-status` replays missed `Done → Completed` bumps (e.g. when a spec's YAML was unreadable at pull time, or it flipped to Done after the merge landed).

---

## Doctor: detect + heal drift

```bash
aida doctor            # multi-agent state-drift diagnostics
aida doctor --heal     # apply fixes
aida doctor --json     # machine-readable
```

Checks include: node/block registry consistency, orphan-branch health, **dead-PID
agent-registry + session-lease reaping** (STORY-496), finder ledgers.

> Run `aida doctor` after a crash, before a release, and when a teammate reports something "stuck."

---

## Telemetry: local-only, privacy-floored

Every `aida` invocation appends one JSONL line to `~/.aida/usage.jsonl`:

- Records: **command shape** (`queue list`), `args_count`, `exit_code`, `duration_ms`.
- Never records: argument **values**, file **paths**, requirement **content**.
- **Local only** — never phoned home.

```bash
aida usage                 # top-20 commands, last 30d
aida usage --unused 30d    # deprecation candidates
aida usage --errors        # high error-rate commands
AIDA_TELEMETRY=0           # opt out (or [telemetry] enabled = false)
```

---

## Multi-user option: PostgreSQL (opt-in)

For a shared-server deployment instead of git-canonical:

```bash
cargo build --features postgres
aida --file "postgres://user:pass@host:5432/aida" list
```

- Backed by `aida-server` (REST + gRPC, port 8080) + the React dashboard.
- Native multi-reader with row-level write locking.

> Default and recommended remains **git-canonical** — vendor-neutral, no server to operate. Reach for PostgreSQL only when a central server is a hard requirement.

---

## Shipping it safely

Before publishing AIDA (or an AIDA-based pack) through any marketplace/registry, run
`docs/security/marketplace-publication-checklist.md`. It covers:

- **Provenance** — canonical repo, pinned version/commit, no unaudited mutable downloads.
- **Filesystem writes** — every project-local path documented; no silent overwrites.
- **MCP tool exposure** — least-privilege default; destructive/operator tools off by default.
- **Secrets** — no embedded creds; tokens via config, never logged.
- **Auditability** — who changed which spec, via which tool, when (commit trailers + traces).
- **Validation** — `aida doctor` / `aida status` on a fresh project pre-publish.

---

## Admin runbook (quick reference)

```bash
# stand up
aida init                          # then commit, push (publishes the aida-store branch)

# onboard a teammate
git clone <repo>; cd <repo>; aida list   # auto-attaches; `aida node acquire` to write

# daily health
aida cache status; aida db status; aida doctor

# recover
aida cache rebuild                 # cache looks wrong
aida db reconcile-status --dry-run # specs stuck at Done
aida doctor --heal                 # dead leases / registry drift

# pin identity (multi-node)
export AIDA_USER=<you>             # in shell rc, every machine
```

<small>Internals: `aida-developer-deep-dive`. Daily use: `aida-user-walkthrough`.</small>
