# Getting Started with AIDA

AIDA is an AI-native requirements management system with a CLI, web dashboard, and desktop app.

**Related Documentation:**
- [User Guide](user-guide.md) — Full reference for all features
- [Administrator's Guide](admin-guide.md) — Storage backends, migration, multi-user setup
- [Developer's Guide](DEVELOPER_GUIDE.md) — For developers maintaining and extending AIDA

---

## 1. Install

Build from source (requires Rust toolchain):

```bash
git clone https://github.com/joemooney/aida.git
cd aida
cargo build --workspace --release
```

This produces binaries in `target/release/`:
- `aida` — Command-line interface
- `aida-server` — REST API + gRPC server
- `aida-gui` — Native desktop app

The web dashboard (`aida-web-react/`) requires Node.js — see [Launching the Web Dashboard](#4-launch-the-web-dashboard) below.

---

## 2. Initialize a Project

Run `aida init` in your project directory:

```bash
cd my-project
aida init
```

This creates:

| File/Directory | Purpose |
|---------------|---------|
| `requirements.db` | SQLite database with seeded META requirements |
| `CLAUDE.md` | Project context for AI sessions |
| `.mcp.json` | Claude Code MCP server integration |
| `.claude/skills/` | 15 workflow skills (`/aida-req`, `/aida-commit`, etc.) |
| `.claude/commands/` | Slash commands for Claude Code |
| `.claude/hooks/` | Commit validation hooks |
| `docs/plans/` | Implementation plan archive |

**Options:**

```bash
aida init --no-skills    # Skip .claude/skills/ and .claude/commands/
aida init --no-hooks     # Skip .claude/hooks/ and git hooks
aida init --force        # Overwrite existing files
```

---

## 3. First Steps with the CLI

**Add your first requirement:**

```bash
aida add --title "User authentication" --type story --status draft
```

**Or use interactive mode:**

```bash
aida add --interactive
```

**List requirements:**

```bash
aida list
aida list --status draft
aida list --type story
```

**View details:**

```bash
aida show STORY-001
```

**Edit a requirement:**

```bash
aida edit STORY-001 --status approved --priority high
```

**Add a comment:**

```bash
aida comment add STORY-001 "Decided to use JWT tokens for auth"
```

**Search:**

```bash
aida search "authentication"
aida grep "auth.*token" -i
```

For the full CLI reference, see the [User Guide — CLI Usage](user-guide.md#cli-usage).

---

## 4. Launch the Web Dashboard

The React web dashboard is the primary UI for most users. It provides kanban boards, sprint planning, advanced filtering, AI chat, and keyboard shortcuts.

**Prerequisites:** Node.js 18+ and pnpm (or npm)

```bash
# Terminal 1: Start the REST API server
cd aida-server
cargo run
# Runs on http://localhost:8080

# Terminal 2: Start the React dashboard
cd aida-web-react
pnpm install    # or: npm install
pnpm dev        # or: npm run dev
# Opens at http://localhost:5173
```

The Vite dev server proxies `/api` requests to the REST API on port 8080.

**Web dashboard views:**
- **Dashboard** — Project-wide status cards, active sprint summary, queue widget
- **Kanban Board** — Drag-and-drop status columns with tag filtering
- **List View** — Flat/tree toggle, advanced query builder, drag-to-queue
- **My Queue** — Personal focus inbox with drag-to-reorder
- **My Activity** — Planned vs. actual work reconciliation
- **Sprint Planning** — Drag-and-drop backlog/sprint assignment
- **Timeline** — Chronological event feed
- **Chat** — AI-powered Q&A with streaming responses (requires API key)
- **Settings** — Store metadata, type definitions, admin controls

**Keyboard shortcuts:** Press `?` in the dashboard to see all shortcuts. Highlights: `g+d` Dashboard, `g+b` Kanban, `g+l` List, `j/k` row navigation, `/` search.

---

## 5. Launch the Desktop App

The native desktop app (egui-based) is an alternative to the web dashboard:

```bash
aida-gui
```

Or open a specific database:

```bash
aida-gui --file /path/to/requirements.db
```

For desktop app features and shortcuts, see the [User Guide — Desktop App](user-guide.md#desktop-app-aida-gui).

---

## 6. Using with Claude Code

If you ran `aida init`, your project is already configured for Claude Code integration.

**Key skills:**

| Skill | Purpose |
|-------|---------|
| `/aida-onboard` | Interactive project walkthrough — start here |
| `/aida-req` | Add a new requirement with AI evaluation |
| `/aida-implement` | Implement a requirement with traceability |
| `/aida-commit` | Commit with automatic requirement linking |
| `/aida-sprint` | Sprint planning |
| `/aida-standup` | Daily standup from recent commits |
| `/aida-search` | Unified search across requirements and code |

**MCP server** (for native tool integration):

The `.mcp.json` created by `aida init` configures Claude Code to use AIDA's MCP server, exposing tools like `list_requirements`, `show_requirement`, `add_requirement`, and `search_requirements` directly.

---

## 7. Storage Backends

`aida init` creates a SQLite database by default. AIDA also supports YAML and PostgreSQL:

```bash
# Migrate to PostgreSQL for team use
aida db migrate --from sqlite --to postgres --output "postgres://user:pass@host:5432/aida"

# Use PostgreSQL directly
aida --file "postgres://user:pass@localhost:5432/aida" list

# Export back to YAML if needed
aida db migrate --from sqlite --to yaml
```

For full details on storage administration, see the [Administrator's Guide](admin-guide.md).

---

## Next Steps

- **Organize requirements:** Create folders and features to group requirements
- **Set up relationships:** Link parent/child, dependency, and verification relationships
- **Plan a sprint:** Use `/aida-sprint` or the Sprint Planning view in the web dashboard
- **Customize AI prompts:** Edit META requirements to tune AI evaluation criteria
- **Export/import templates:** Share requirement hierarchies between projects with `aida export --format tree`

See the [User Guide](user-guide.md) for comprehensive documentation on all features.
