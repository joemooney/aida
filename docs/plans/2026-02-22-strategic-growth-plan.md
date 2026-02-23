# Strategic Growth Plan: AIDA Next Phase

**Date:** 2026-02-22
**Status:** Draft — Awaiting Review

## Related Requirements
- Addresses growth areas identified in `docs/WHY-AIDA.md`
- Bootstrapping, auth, Docker, YAML/MCP strategy, CLAUDE.md management

---

## 1. Bootstrapping New Projects

### Current State

`aida scaffold apply` generates ~30+ files: CLAUDE.md, 15 skills, 13 commands, hooks, .mcp.json, git hooks. This is the right default.

### Why Not Progressive Tiers

An earlier draft proposed hiding skills and hooks behind opt-in flags to reduce "overwhelm." On reflection, that's solving an imagined problem by removing the features that make AIDA worth adopting.

The skills are **text files**. They cost nothing at runtime, change no behavior unless invoked, and are the primary way users discover AIDA's workflow. A user who runs `aida init` and only gets a database + MCP config would never find `/aida-req`, `/aida-commit`, or `/aida-capture` — which is where the actual value lives. Without skills, AIDA is just another requirements database.

The real adoption friction isn't "too many files." It's **not understanding what those files do**. The fix is better onboarding, not fewer files.

### Proposed: Full Scaffold + Better First-Run Guidance

`aida init` continues to scaffold everything. What changes is the post-init experience:

```
AIDA initialized. Created:

  requirements.db          Requirements database
  .mcp.json                Claude Code MCP integration (AI can query requirements)
  CLAUDE.md                Project context for AI sessions
  .claude/skills/          15 workflow skills (/aida-req, /aida-commit, etc.)
  .claude/commands/        13 slash commands
  .claude/hooks/           Commit validation and traceability hooks
  docs/plans/              Implementation plan archive

Quick start:
  aida add --title "User authentication" --type story --status draft
  aida list

In Claude Code:
  Type /aida- to see available workflow skills
  Try /aida-onboard for an interactive project walkthrough

Full docs: aida user-guide
```

**Key improvements:**
1. **Explain what was created and why** — not just a file list, but what each piece does.
2. **Point to `/aida-onboard`** — the interactive onboarding skill that already exists. This is the real first-run experience: an AI-guided tour of the project.
3. **Show the discovery path** — "type `/aida-` to see skills" teaches the user how to find things organically.

### Optional Flags (Subtract, Don't Add)

For users who genuinely want less (e.g., using AIDA with a non-Claude AI tool where skills are irrelevant):

```bash
aida init --no-hooks       # Skip commit validation hooks
aida init --no-skills      # Skip Claude Code skills/commands
```

These are escape hatches, not the recommended path. The default gives you everything.

---

## 2. User Management & Authentication

### Current State

No authentication whatsoever. User identity is a plain string (`owner` field, `?user=` URL param, `X-User-Agent` header). Anyone can read or modify anything. This is fine for solo use, problematic for teams.

### Design Principles

1. **Auth should be optional.** Solo developers and local-only setups should never be forced through a login screen.
2. **Auth should be pluggable.** Different organizations have different identity providers.
3. **Start with the simplest useful thing.** Don't build RBAC before you build login.

### Proposed: Three-Layer Authentication Architecture

#### Layer 0: No Auth (Default — Current Behavior)

Server starts without any auth config. All endpoints are open. User identity is honor-system via headers. This remains the default for `aida-server` and local development.

#### Layer 1: API Key Authentication

The simplest real auth. Good for small teams and CI/CD integration.

```toml
# .aida/server.toml
[auth]
mode = "api-key"

[auth.api_keys]
joe = "aida_key_abc123..."
alice = "aida_key_def456..."
```

Implementation:
- Server checks `Authorization: Bearer <key>` header
- Maps key to username (used as `owner` identity)
- No key = 401 Unauthorized (when auth mode is enabled)
- Keys can be managed via CLI: `aida auth add-key --user joe`

This is enough for: CI/CD pipelines, remote CLI access, multi-user web dashboard where you trust your network but want identity.

#### Layer 2: OpenID Connect / OAuth 2.0

For organizations with existing identity providers. Supports:
- **Azure AD / Entra ID** (corporate Windows/Active Directory login)
- **Google Workspace**
- **Okta, Auth0, Keycloak**
- **Any OIDC-compliant provider**

```toml
# .aida/server.toml
[auth]
mode = "oidc"

[auth.oidc]
issuer_url = "https://login.microsoftonline.com/{tenant-id}/v2.0"
client_id = "aida-app-id"
client_secret_env = "AIDA_OIDC_SECRET"  # Read from env var
scopes = ["openid", "profile", "email"]
# Map OIDC claims to AIDA identity
username_claim = "preferred_username"    # or "email", "upn", etc.
```

Implementation:
- Server validates JWT tokens from the OIDC provider
- Extracts username from configured claim
- Web dashboard redirects to OIDC login page
- CLI uses device code flow or token refresh
- No user database needed — identity comes from the provider

**Why OIDC over raw Active Directory/LDAP:**
- OIDC works with Azure AD (which is the modern AD interface) without needing LDAP
- It also works with every other major identity provider
- It's HTTP-based (no LDAP protocol complexity)
- The `openidconnect` Rust crate is mature and well-maintained

#### Layer 3: Role-Based Access Control (Future)

Once auth exists, RBAC can be layered on:

```toml
[auth.roles]
admin = ["joe"]         # Full access
editor = ["alice", "bob"]  # CRUD on requirements
viewer = ["*"]          # Read-only (default role)
```

This is explicitly **not in the first implementation**. Get auth working first, then add authorization.

### Active Directory / Windows Login Specifically

The user asked about this. The answer is: **use OIDC with Azure AD (Entra ID)**, not raw LDAP/Kerberos. Here's why:

- Azure AD exposes standard OIDC endpoints
- Most organizations with Active Directory also have Azure AD (or are migrating)
- OIDC is simpler to implement and maintain than LDAP binds
- Works for both on-prem AD (via Azure AD Connect) and cloud-only
- The web dashboard can use the standard OAuth redirect flow
- The CLI can use device code flow (like `az login`)

If someone genuinely needs on-prem LDAP without Azure AD (rare but possible), that would be a separate auth provider implementation using the `ldap3` Rust crate, added later.

### Implementation Priority

1. **API Key auth** (2-3 days) — Covers CI/CD, remote CLI, small teams
2. **OIDC** (1-2 weeks) — Covers enterprise, Azure AD, Google
3. **RBAC** (later) — Only after real users request specific access controls

---

## 3. Docker Story

### Current State

Docker setup exists in `docker/` with docker-compose.yml defining 8 services (Traefik, GitLab, PostgreSQL, pgAdmin, Cloudflare tunnel, AIDA server, AIDA web). This is a production deployment config, not a getting-started experience.

### The Problem

There are actually **three different Docker use cases**, and they need different things:

1. **"I want to try AIDA"** — Developer wants to spin up the web dashboard in 30 seconds
2. **"I want to deploy for my team"** — Small team wants a persistent server with a database
3. **"I want production infrastructure"** — Organization wants TLS, auth, monitoring, backups

The current docker-compose.yml targets use case 3. Use cases 1 and 2 have no support.

### Proposed: Three Docker Configurations

#### Quick Start (`docker-compose.quick.yml`)

```yaml
services:
  aida:
    image: ghcr.io/joemooney/aida:latest
    ports:
      - "8080:8080"    # REST API
      - "5173:5173"    # Web dashboard
    volumes:
      - ./data:/data   # Persistent requirements data
    environment:
      - AIDA_DEV_MODE=false
```

One service, two ports, one volume. Run `docker compose -f docker-compose.quick.yml up` and open `localhost:5173`. Data persists in `./data/`. That's it.

**Pre-built image:** Publish `ghcr.io/joemooney/aida:latest` via GitHub Actions. Contains both `aida-server` and the React dashboard served by the same process (or a minimal nginx sidecar in the same container).

#### Team Deployment (`docker-compose.team.yml`)

```yaml
services:
  aida-server:
    image: ghcr.io/joemooney/aida:latest
    ports:
      - "8080:8080"
    volumes:
      - aida-data:/data
    environment:
      - AIDA_AUTH_MODE=api-key
      - AIDA_DATABASE_URL=postgres://aida:${PG_PASSWORD}@postgres:5432/aida
    depends_on:
      - postgres

  aida-web:
    image: ghcr.io/joemooney/aida-web:latest
    ports:
      - "80:80"
    environment:
      - API_URL=http://aida-server:8080

  postgres:
    image: postgres:16-alpine
    volumes:
      - pg-data:/var/lib/postgresql/data
    environment:
      - POSTGRES_DB=aida
      - POSTGRES_USER=aida
      - POSTGRES_PASSWORD=${PG_PASSWORD}

volumes:
  aida-data:
  pg-data:
```

Three services: server, web, database. PostgreSQL for real concurrent access. API key auth ready. Still simple to deploy.

#### Production (`docker-compose.yml` — Current)

Keep the existing config with Traefik, Cloudflare, GitLab, pgAdmin, etc. This is for organizations that need the full infrastructure story.

### Combined Image Strategy

For the quick-start experience, consider building a **single container** that runs both the REST API server and serves the React dashboard:

- `aida-server` already serves REST on port 8080
- Add static file serving for the React build output (Axum's `ServeDir`)
- Single port, single container, zero nginx needed
- The team/production configs can split them for scalability

### Publish to Container Registry

Set up GitHub Actions to:
1. Build `aida-server` with `--features postgres`
2. Build `aida-web-react` (production build)
3. Package into `ghcr.io/joemooney/aida:latest` and `:v{version}`
4. Publish on every tagged release

This makes `docker run ghcr.io/joemooney/aida` a real command that works.

---

## 4. YAML Representation vs. MCP/Skills Strategy

### Current State

- SQLite is the active backend (`requirements.db`, 1.7 MB)
- YAML exists but is stale/migrated (`requirements.yaml`, 978 KB)
- MCP server works with either backend transparently
- Skills use CLI commands (`aida list`, `aida show`), which auto-detect the backend
- Release workflow exports SQLite → YAML for git-friendly diffs

### The Question

Should AIDA always maintain a YAML file alongside the database so AI agents can read it directly?

### The Answer: No — But With a Nuance

**The MCP/CLI approach is the right primary strategy.** Here's why:

1. **Context window efficiency.** A 978 KB YAML file would consume ~250K tokens if pasted into a context window. The MCP server lets the AI fetch only what it needs: `show_requirement FR-0042` returns one requirement, not 500.

2. **Consistency.** A dual-write system (every DB change also writes YAML) is a maintenance burden and a source of drift bugs. Single source of truth is better.

3. **Performance.** SQLite with WAL mode handles concurrent access from CLI + web dashboard + MCP server. YAML with file locking doesn't.

4. **Backend agnosticism.** Skills and MCP already work with any backend. Adding a YAML-always requirement would couple everything to YAML.

### The Nuance: YAML as a Snapshot Format

YAML isn't useless — it's great for specific purposes:

1. **Git history.** Export to YAML on release so requirement changes are visible in git diffs. This is already in the release workflow.

2. **Bootstrapping.** A new project could start from a YAML template imported from another project (tree export/import already supports this).

3. **Offline AI context.** If someone wants to share their full requirements state with an AI that doesn't have MCP access (e.g., pasting into a web chat), a YAML or markdown export is useful.

4. **Backup.** `aida db migrate --from sqlite --to yaml` is a human-readable backup.

### Recommended Enhancement: `aida export --format context`

Add a new export format designed specifically for AI context injection:

```bash
aida export --format context > requirements-context.md
```

This would generate a **markdown summary** optimized for AI consumption:
- Requirement counts by type/status
- Active sprint contents
- Recently modified requirements
- Open items by owner
- Key relationships graph

This is more useful than raw YAML because it's curated for relevance, not completeness. It could be auto-appended to CLAUDE.md or used as an MCP resource.

---

## 5. CLAUDE.md Management Strategy

### Current State

CLAUDE.md is generated by `aida scaffold apply` but intentionally has **no AIDA header** — it's treated as a user-editable document. The generation includes project metadata, tech stack, skills documentation, and workflow guidance. The `/aida-sync` skill can detect drift between CLAUDE.md and actual templates.

### The Tension

CLAUDE.md serves two masters:
1. **The human** who adds project-specific notes, architecture decisions, and conventions
2. **The system** that needs it to reflect current skills, commands, and project state

Right now, the human wins — CLAUDE.md is manually maintained and `aida scaffold apply --force` is needed to overwrite. This means CLAUDE.md drifts from reality.

### Proposed: Managed Sections in CLAUDE.md

Split CLAUDE.md into **managed sections** (auto-updated) and **user sections** (never touched):

```markdown
# CLAUDE.md

## Project Overview
<!-- USER SECTION: Edit freely, AIDA will not modify this -->
Your custom project description, architecture notes, conventions...

## Requirements Management
<!-- AIDA MANAGED: Auto-updated by `aida sync claude-md` -->
<!-- Last synced: 2026-02-22T15:30:00Z -->
This project uses AIDA for requirements tracking...
[Auto-generated content about DB backend, CLI commands, etc.]
<!-- END AIDA MANAGED -->

## Claude Code Skills
<!-- AIDA MANAGED: Auto-updated by `aida sync claude-md` -->
Available skills: /aida-req, /aida-implement, ...
[Auto-generated skills list with descriptions]
<!-- END AIDA MANAGED -->

## Code Traceability
<!-- AIDA MANAGED -->
[Auto-generated traceability guidelines]
<!-- END AIDA MANAGED -->

## Project Architecture
<!-- USER SECTION -->
Your custom architecture notes...

## Development Notes
<!-- USER SECTION -->
Your conventions, decisions, etc...
```

### Implementation

New command: `aida sync claude-md`

1. Reads existing CLAUDE.md
2. Identifies managed sections by `<!-- AIDA MANAGED -->` / `<!-- END AIDA MANAGED -->` markers
3. Regenerates only managed sections from current project state
4. Preserves everything else exactly as-is
5. Updates the "Last synced" timestamp

This can be:
- Run manually: `aida sync claude-md`
- Run automatically via the `/aida-sync` skill
- Run as a git hook (post-commit, to keep it fresh)
- Run as part of `aida scaffold apply`

### What Gets Auto-Managed

| Section | Source | Update Trigger |
|---------|--------|---------------|
| Requirements Management | DB backend type, CLI commands | Backend change, migration |
| Claude Code Skills | Embedded template list | Template updates, scaffold apply |
| Code Traceability | Commit format config | Config changes |
| Requirement Types | Type definitions in DB settings | Settings changes |
| Project Summary Stats | DB query (counts by status/type) | Each sync |

### What Stays User-Controlled

- Project Overview / description
- Architecture decisions
- Development conventions
- Team notes
- Any section without AIDA MANAGED markers

### Benefits

- CLAUDE.md stays accurate without manual maintenance
- User customizations are never lost
- New skills/commands appear automatically after template updates
- Project stats stay current (useful for AI context)

---

## 6. Addressing the Growth Areas (from WHY-AIDA.md)

### 6.1 Adoption Friction → Progressive Bootstrapping (Section 1 above)

Three tiers, zero-config default, MCP-first approach.

### 6.2 Team Collaboration → Auth + Notifications

**Phase 1: Identity** (Section 2 above)
- API key auth for small teams
- OIDC for enterprise

**Phase 2: Notifications** (Future)
- Webhook system: requirement status changes → HTTP POST to configured endpoints
- This enables Slack integration, CI/CD triggers, email notifications
- Implementation: event bus in `aida-core` that fires on any mutation

```toml
# .aida/server.toml
[[webhooks]]
url = "https://hooks.slack.com/services/..."
events = ["requirement.status_changed", "sprint.completed"]
```

**Phase 3: Real-time** (Future)
- WebSocket support for live dashboard updates
- Server-Sent Events (SSE) for simpler real-time feeds
- Optimistic locking already prevents conflicts; real-time just makes the UI responsive

### 6.3 Integration Breadth → Ecosystem Connectors

Priority order based on user value:

1. **GitHub Integration** (highest value — most AIDA users are on GitHub)
   - Mirror of existing GitLab integration pattern
   - Bidirectional sync: requirement ↔ GitHub Issue
   - PR linking: commit spec IDs auto-link to issues
   - GitHub Actions: `aida` CLI in CI for requirement status updates

2. **Slack/Discord** (via webhooks — see 6.2 above)
   - Webhook-based, no custom integration needed initially
   - Later: Slash commands (`/aida show FR-0042`) via bot

3. **CI/CD** (via CLI in pipelines)
   - `aida` CLI runs in GitHub Actions / GitLab CI
   - `aida list --status in-progress --format json` for pipeline decisions
   - `aida edit FR-0042 --status completed` for auto-status updates

### 6.4 Reporting and Analytics

**Quick wins:**
- `aida stats` CLI command: requirement counts, velocity, churn
- Dashboard "Analytics" view: charts for velocity, completion rate, type distribution over time
- Export to CSV/JSON for external analysis

**Medium-term:**
- AI contribution metrics: percentage of code with `trace:` + `ai:claude` annotations
- Quality score trends: track AI evaluation scores over time
- Sprint velocity: story points completed per sprint (requires point estimation)

### 6.5 Non-Claude AI Support

**MCP is already the bridge.** The MCP protocol is being adopted by multiple AI tools. As long as `aida mcp-serve` works, any MCP-compatible client can use it.

**Additional targets:**
- **Cursor:** Generate `.cursor/rules` from AIDA project state (similar to CLAUDE.md generation)
- **Windsurf:** Generate `.windsurfrules` equivalent
- **Generic:** `aida export --format ai-context` generates a universal AI context file

**Principle:** The database and MCP server are tool-agnostic. Only the skill files and CLAUDE.md are Claude-specific. Generating equivalent config for other tools is a template exercise, not an architecture change.

### 6.6 Documentation and Marketing

**Immediate:**
- Getting Started tutorial (5-minute version using Tier 1 bootstrapping)
- Video walkthrough of the web dashboard
- Comparison table (AIDA vs Jira vs Linear vs GitHub Issues)

**Medium-term:**
- Landing page (could be a GitHub Pages site generated from docs/)
- "AIDA for Teams" guide (auth, Docker, PostgreSQL setup)
- "AIDA for Regulated Industries" guide (traceability, audit, compliance)

---

## Implementation Priority

Sequenced by value delivered per effort:

| Priority | Item | Effort | Impact |
|----------|------|--------|--------|
| 1 | Better first-run guidance + onboarding polish | 1-2 days | Removes adoption confusion without removing features |
| 2 | CLAUDE.md managed sections | 2-3 days | Keeps AI context fresh automatically |
| 3 | Docker quick-start image | 2-3 days | "Try AIDA in 30 seconds" |
| 4 | `aida export --format context` | 1 day | Better AI context for non-MCP scenarios |
| 5 | API key authentication | 2-3 days | Enables multi-user without complexity |
| 6 | GitHub integration | 1 week | Biggest ecosystem gap |
| 7 | Webhook system | 3-4 days | Enables Slack, CI/CD, notifications |
| 8 | OIDC authentication | 1-2 weeks | Enterprise readiness |
| 9 | Analytics dashboard view | 1 week | PM story |
| 10 | Non-Claude AI configs | 3-4 days | Broader audience |

---

## Open Questions

1. **Default storage:** Should `aida init` create SQLite (better for MCP/concurrent) or YAML (simpler, visible)? Recommendation: SQLite, since the MCP server and web dashboard both benefit from it.

2. **CLAUDE.md section markers:** Should we use HTML comments (`<!-- AIDA MANAGED -->`) or a custom syntax? HTML comments are invisible in rendered markdown, which is clean but means users might accidentally delete them.

3. **Single vs. split Docker image:** One container serving both API and web, or separate containers? Single is simpler for quick-start; split is better for team/production. Could do both.

4. **Auth migration path:** When a team enables auth on an existing server, what happens to existing `owner` fields that don't match authenticated usernames? Need a migration/mapping strategy.

5. **GitHub integration scope:** Full bidirectional sync (like GitLab) or simpler one-way push (AIDA → GitHub Issues)? Bidirectional is more valuable but significantly more complex.
