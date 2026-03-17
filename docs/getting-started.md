# Getting Started with AIDA

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

```bash
# Linux x86_64
curl -L https://github.com/joemooney/aida/releases/latest/download/aida-linux-x86_64 -o ~/.local/bin/aida
chmod +x ~/.local/bin/aida

# macOS (Apple Silicon)
curl -L https://github.com/joemooney/aida/releases/latest/download/aida-darwin-arm64 -o /usr/local/bin/aida
chmod +x /usr/local/bin/aida
```

> **Note**: Pre-built binaries will be available once GitHub releases are set up. Until then, use cargo install or build from source.

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
aida init
```

This creates:
- `requirements.db` — SQLite database for requirements
- `CLAUDE.md` — Project context for AI assistants
- `.claude/skills/` — AIDA workflow skills for Claude Code
- `.mcp.json` — MCP integration for Claude Code
- `docs/plans/` — Implementation plan archive

That's it. You're ready to use AIDA.

---

## Step 3: Add Your First Requirement

```bash
aida add --title "User authentication" \
  --type functional \
  --description "Users can log in with email and password" \
  --status draft
```

Or use interactive mode (prompts for each field):

```bash
aida add
```

---

## Step 4: Work With Requirements

```bash
# List all requirements
aida list

# Show details
aida show FR-001

# Edit
aida edit FR-001 --status approved --owner alice

# Search
aida search "authentication"

# Add a comment
aida comment add FR-001 --content "Needs OAuth2 support"

# Add a relationship
aida rel add --from FR-002 --to FR-001 --type references

# Delete
aida del FR-003 --yes
```

---

## What's Next?

### For Solo Developers

You're done. The SQLite database + YAML export gives you everything you need. A pre-commit hook auto-exports `requirements.yaml` for git-diffable history.

### For Teams (Multi-User)

Two options depending on your connectivity:

**Always connected** — use PostgreSQL:
```bash
# Start PostgreSQL (Docker)
docker run -d --name aida-pg -p 5432:5432 \
  -e POSTGRES_USER=aida -e POSTGRES_PASSWORD=aida \
  -e POSTGRES_DB=aida_default postgres:15-alpine

# Migrate your data
aida db migrate --from sqlite --to postgres \
  --output "postgres://aida:aida@localhost:5432/aida_default"

# Start the server (REST API + web dashboard)
aida-server --database "postgres://aida:aida@localhost:5432/aida_default" \
  --host 0.0.0.0 --rest-port 8080

# Other team members connect via:
#   CLI:  aida --file "postgres://aida:aida@<your-ip>:5432/aida_default" list
#   Web:  http://<your-ip>:8080
```

**Sometimes offline** — use distributed mode:
```bash
aida init --distributed
# Creates an orphan branch 'aida-store' with sharded YAML files
# Every mutation auto-commits to the store branch
# Sync with: git push origin aida-store
# New team member setup: git worktree add .aida-store aida-store
```

See [storage-modes.md](storage-modes.md) for a full comparison of all 5 storage options.
See [multi-user-setup.md](multi-user-setup.md) for detailed PostgreSQL multi-user instructions.

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
| `aida init` | Initialize AIDA in current project |
| `aida init --distributed` | Initialize with git-backed distributed store |
| `aida list` | List all requirements |
| `aida add --title "..." --type functional` | Add a requirement |
| `aida show FR-001` | Show requirement details |
| `aida edit FR-001 --status approved` | Update a requirement |
| `aida search "keyword"` | Search requirements |
| `aida comment add FR-001 --content "..."` | Add a comment |
| `aida rel add --from FR-002 --to FR-001 --type references` | Add relationship |
| `aida del FR-001 --yes` | Delete a requirement |
| `aida db info` | Show database info |
| `aida db status` | Show distributed store status |
| `aida db merge-gate` | Assign short agreed IDs |
| `aida db sync --pull --push` | Sync with remote |
| `aida github push FR-001` | Push to GitHub issue |
| `aida github pull` | Import GitHub issues |

---

## Uninstall

```bash
# Remove from a project
rm -f requirements.db requirements.yaml .mcp.json
rm -rf .aida/ .claude/skills/aida-* .claude/commands/aida-* docs/plans/

# Remove the binary
rm -f ~/.cargo/bin/aida          # if installed via cargo
rm -f ~/.local/bin/aida          # if installed manually

# Remove distributed store (if used)
git worktree remove .aida-store  # remove worktree
git branch -D aida-store         # remove orphan branch
```

AIDA doesn't modify your source code or git history (unless you add trace comments). Removing it is clean.
