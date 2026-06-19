# Getting Started with AIDA

<!-- trace:TASK-886 -->

Add AI-native requirements tracking to any project in under 5 minutes.

---

## Step 1: Install AIDA

Choose the method that works for you, from simplest to most flexible:

### Option A: Cargo Install (Recommended)

If you have Rust installed:

```bash
cargo install --git https://github.com/joemooney/aida.git aida-cli
```

This builds and installs the `aida` binary to `~/.cargo/bin/` (already in your PATH if you use rustup).

Verify:
```bash
aida --version
```

### Option B: Download Pre-built Binary

Each release publishes per-platform tarballs (`aida-{linux,darwin}-{x86_64,arm64}.tar.gz`):

```bash
# Linux x86_64
curl -L https://github.com/joemooney/aida/releases/latest/download/aida-linux-x86_64.tar.gz \
  | tar -xz -C ~/.local/bin aida
chmod +x ~/.local/bin/aida

# macOS (Apple Silicon)
curl -L https://github.com/joemooney/aida/releases/latest/download/aida-darwin-arm64.tar.gz \
  | tar -xz -C /usr/local/bin aida
chmod +x /usr/local/bin/aida
```

Other targets: `aida-linux-arm64.tar.gz`, `aida-darwin-x86_64.tar.gz`. Verify with `aida --version`.

### Option C: Docker (No Installation)

Run AIDA without installing anything:

```bash
# Add an alias to your shell
alias aida='docker run --rm -v "$(pwd):/repo" -w /repo ghcr.io/joemooney/aida'

# Then use normally
aida init
aida list
```

> **Note**: Docker image will be available once CI publishes it. Until then, build locally: `docker build -t aida .` from the AIDA repo.

### Option D: Build from Source

```bash
git clone https://github.com/joemooney/aida.git ~/aida
cd ~/aida
cargo build --release -p aida-cli
cp target/release/aida ~/.local/bin/
```

### Don't Have Rust?

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
# Then use Option A
```

---

## Step 2: Initialize Your Project

Navigate to your project and run:

```bash
cd ~/my-project
git init           # if not already a git repo
aida init
```

This creates (the **distributed git-canonical** default):
- `aida-store` orphan git branch with worktree at `.aida-store/` (gitignored) — canonical store, one YAML file per requirement
- `.aida/config.toml` — distributed-mode marker
- `.aida/cache.db` (gitignored) — SQLite read cache, auto-rebuilt
- META requirements seeded into the orphan store
- `CLAUDE.md` — project context for AI assistants
- `AGENTS.md` — project context for Codex-compatible agents
- `.claude/skills/`, `.claude/commands/`, `.claude/hooks/` — Claude Code workflow scaffolding (by default also scaffolds the Codex profile; `--agent claude` for Claude only)
- `.mcp.json` — MCP integration
- `docs/plans/` — implementation plan archive
- `docs/aida/discipline/` — the starter discipline pack (habits + vocabulary for running an AIDA project)
- the default role set (implementer, advisor, reviewer) bootstrapped into `~/.aida/roles/` so a fresh machine is ready out of the box (skip with `--no-roles`)

That's it. You're ready to use AIDA.

> **Need the legacy SQLite-canonical mode?** Pass `--centralized` (deprecated, prints a warning). For multi-repo workspaces use `--sibling` instead. To skip the agent scaffolding entirely, use `--no-skills` / `--no-hooks`.

> **Cloning a project that already uses AIDA?** Don't run `aida init` — see [Joining an existing AIDA project](#joining-an-existing-aida-project) below.

---

## Joining an existing AIDA project

If you cloned a repo that **already** uses AIDA (the `aida-store` orphan branch exists on the remote), you do **not** run `aida init`. The first store-reading command auto-attaches the store for you:

```bash
git clone git@github.com:org/their-project.git
cd their-project
aida list           # auto-attaches the .aida-store/ worktree and rebuilds the cache
```

That first `aida list` (or `aida search`, `aida queue`, …) attaches the `.aida-store/` worktree from the `aida-store` branch and rebuilds the local `.aida/cache.db` — no manual setup. Reading works immediately.

To **write** new requirements from your clone, you need a node id (the namespace for new spec IDs until the merge gate collapses them to short IDs):

```bash
aida node acquire   # claims a node id for this clone
```

> **Offline or store unreachable?** If distributed mode is declared but the store branch can't be attached (you're offline, the remote lacks `aida-store`, etc.), reads error with setup guidance rather than silently falling back to a legacy file — so you always know the store isn't really there.

---

## Step 3: Add Your First Requirement

The simplest way — just describe it in a line:

```bash
aida add "Users can log in with email and password"
```

That captures it as a **task** (the default) with a stable id like `TASK-1`.
Want a different type or more detail? Use the flags:

```bash
aida add --title "User authentication" \
  --type functional \
  --description "Users can log in with email and password" \
  --status draft
```

Or run `aida add` with no arguments for an interactive walkthrough that prompts
for each field.

---

## Step 4: Work With Requirements

**The loop: capture → build → link → done.** AIDA doesn't build the task for
you — you do that in your editor, the way you always have. AIDA's job is the
*link*: when you reference the spec id in your work, the code and the spec stay
wired together.

```bash
aida add "Build login page"      # → TASK-1
aida list                        # see what's on your plate
# ...now build it in your editor. In the commit that does the work,
#    put the id in the message:  git commit -m "feat: login page (TASK-1)"
#    (or drop a // trace:TASK-1 comment where you implement it)
aida done TASK-1                 # mark it finished
aida show TASK-1                 # ...and now you see your commit linked to the task
```

That last step is the point: `aida show` reveals the code that fulfilled the
spec, automatically, because you named the id. No manual bookkeeping.

### Full command reference

```bash
# List your tasks
aida list

# Mark one done (the simple way)
aida done TASK-1

# Show details — and how its code links back
aida show TASK-1

# Search your tasks (add --include-meta for the seeded AI prompts)
aida search "authentication"

# Edit any field
aida edit TASK-1 --status approved --owner alice

# Add a comment (content is a positional argument — there is no --content flag)
aida comment add TASK-1 "Needs OAuth2 support"

# Add a relationship
aida rel add --from TASK-2 --to TASK-1 --type references

# Delete
aida del TASK-3 --yes
```

---

## What's Next?

### Solo developers / small teams

You're done. The git-canonical store gives you per-requirement YAML files (diffable, mergeable, agent-readable) plus a SQLite cache for fast list/filter/search. Mutations auto-commit to the orphan branch. Push the branch to share:

```bash
cd .aida-store && git push -u origin aida-store
# Or via aida helper:
aida db sync --pull --push
```

### Multi-repo workspaces

If multiple code repos share one requirements store:

```bash
aida init --sibling --registry-remote git@github.com:org/aida-registry.git
# The store lives at ../aida-store/ as its own repo
```

### Teams wanting a server-backed shared projection

Build with the `postgres` feature, run `aida-server` against a Postgres connection string. Note: this is a server-backed projection, **not** the canonical store — git is still the source of truth.

```bash
cargo install --git https://github.com/joemooney/aida.git aida-cli --features postgres
# See docs/multi-user-setup.md for full instructions
```

See [storage-modes.md](storage-modes.md) for the full mode comparison.
See [multi-user-setup.md](multi-user-setup.md) for PostgreSQL deployment details.

### With Claude Code

AIDA is designed for AI-assisted development. If you use Claude Code, the skills are available immediately after `aida init`:

```
/aida-req          # Add a requirement (AI-assisted quality feedback)
/aida-implement    # Implement a requirement with traceability
/aida-commit       # Commit with requirement linking
/aida-capture      # End-of-session requirement capture
/aida-onboard      # Interactive project walkthrough
```

The MCP server lets Claude Code query your requirements database directly — already configured by `aida init`.

### With GitHub

```bash
# Configure (one time)
export AIDA_GITHUB_TOKEN="ghp_..."
aida github config --repo your-org/your-project

# Push a requirement as a GitHub issue
aida github push FR-001

# Import GitHub issues as requirements
aida github pull

# Create AIDA labels in your repo
aida github labels --create-missing
```

---

## Quick Reference

| Command | What it does |
|---------|-------------|
| `aida init` | Initialize AIDA in current project (default: distributed git-canonical) |
| `aida init --sibling` | Initialize for multi-repo workspace (sibling-repo store) |
| `aida init --centralized` | Legacy SQLite mode (deprecated) |
| `aida add "..."` | Capture a task — the simple way |
| `aida add --title "..." --type functional` | Add with an explicit type / fields |
| `aida list` | List your tasks (cache-backed, sub-ms) |
| `aida done TASK-1` | Mark a task done |
| `aida show TASK-1` | Show details + how its code links back |
| `aida edit TASK-1 --status approved` | Update any field |
| `aida search "keyword"` | Cache-backed FTS5 search |
| `aida comment add FR-1-001 "..."` | Add a comment (content is positional) |
| `aida rel add --from FR-1-002 --to FR-1-001 --type references` | Add relationship |
| `aida del FR-1-001 --yes` | Delete a requirement |
| `aida cache status` | Compare cache HEAD vs git HEAD |
| `aida cache rebuild` | Force-rebuild cache from git store |
| `aida db status` | Show distributed store sync state |
| `aida db merge-gate` | Assign short agreed IDs (`FR-7-001` → `FR-1`) |
| `aida db sync --pull --push` | Sync orphan branch with remote |
| `aida github push FR-1-001` | Push to GitHub issue (requires `github` feature) |
| `aida github pull` | Import GitHub issues |

---

## Uninstall

```bash
# Remove distributed-mode store + scaffolding from a project
git worktree remove .aida-store        # remove the worktree
git branch -D aida-store               # remove the orphan branch
rm -rf .aida/ .claude/skills/aida-* .claude/commands/aida-* docs/plans/
rm -f .mcp.json AGENTS.md              # if generated

# Remove legacy centralized files (if you ever used --centralized)
rm -f requirements.db requirements.yaml

# Remove the binary
rm -f ~/.cargo/bin/aida                # if installed via cargo
rm -f ~/.local/bin/aida                # if installed manually
```

AIDA doesn't modify your source code or git history (unless you add trace comments). Removing it is clean.
