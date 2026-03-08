# Plan: Development Workflow & PostgreSQL-First Architecture

**Date**: 2026-03-08

## Context

AIDA's current recommended path is Docker with SQLite, but SQLite occupies an awkward middle ground — can't commit it (binary), needs a pre-commit hook to export to YAML, no better than YAML for solo use, not viable for teams. The natural storage split is:

- **YAML**: Solo/simple, no infrastructure needed, git-friendly
- **PostgreSQL**: Anything beyond solo — teams, multi-branch, persistent server

The cloned AIDA repo becomes the "server home" — housing one PostgreSQL instance that serves all your projects, each in its own database.

## Decisions

### Multi-project: Single PG instance, database per project

One PostgreSQL instance, one database per project. Clean isolation, standard PostgreSQL pattern. The server discovers available databases on startup or via a registry table in a shared `aida_meta` database. Export/import to YAML works via `aida db migrate` for portability.

### Ports: Fixed defaults with env overrides

- **8080** for web/API (already established)
- **5432** for PostgreSQL (standard)

If a port is in use, fail with a clear message: "Port 8080 in use — set AIDA_PORT=8081 in .env or stop the conflicting process." Both ports registered in `~/.ports` during `aida server start`.

### Auth: Tiered by context

| Context | Default | Why |
|---------|---------|-----|
| `make dev` | none | Local development, localhost only |
| `aida server start` (first run) | Auto-generate PIN | Displayed once: "Your PIN is 7294. Save it." Stored in `.aida/server.toml` |
| `aida init` (scaffolded project) | Inherits from server | Project connects to existing server, auth already configured |
| Team/remote | OIDC | Explicit setup via `aida server auth oidc --issuer ...` |

Migration path: start with no auth, run `aida server auth pin` anytime to enable. Upgrade to OIDC with `aida server auth oidc`. Downgrade with `aida server auth none`. Each updates server config and restarts. No data migration — auth is orthogonal to data.

For `aida init`: if a server is running, connect to it (auth already handled). If not, scaffold YAML mode. User can start a server later with `aida server start` and migrate.

## Goals

1. **Fast development workflow** for AIDA contributors (hot-reload, ~5s iteration)
2. **Simple start/stop** for end-users via `aida server start/stop`
3. **PostgreSQL-first** for any project that outgrows YAML
4. **No pollution** of end-user project directories (everything in `.aida/`)

## Architecture

### Storage Tiers

| Tier | Backend | Use Case | Infrastructure |
|------|---------|----------|---------------|
| **Simple** | YAML | Solo dev, small project, git-native | None |
| **Standard** | PostgreSQL | Multi-branch, team, web dashboard | Docker (via AIDA clone or standalone) |

SQLite becomes a legacy/migration path, not a recommended choice.

### Where PostgreSQL Lives

```
~/ai/aida/                          # Cloned AIDA repo
├── .aida/
│   ├── docker-compose.yml          # PostgreSQL + aida-server + web dashboard
│   ├── pgdata/                     # PostgreSQL data (Docker volume mount, .gitignored)
│   ├── server.toml                 # Server config (auth PIN, ports, etc.)
│   └── web-sessions.sqlite3        # Web session store
```

One PostgreSQL instance, multiple databases:
- `aida_default` — AIDA's own requirements (this repo)
- `aida_myproject` — scaffolded project at ~/projects/myproject
- `aida_otherproject` — another scaffolded project

### Scaffolded Project Layout

When `aida init` detects a running server:

```
~/projects/myproject/
├── .aida/
│   ├── config.toml                 # connection_url = "postgres://..."
│   └── (no docker-compose here)    # server lives in the AIDA clone
├── requirements.yaml               # Optional: exported snapshot for git
├── CLAUDE.md
└── .claude/skills/...
```

The scaffolded project stores a connection string pointing to the shared PostgreSQL. No Docker files in the user's project.

### Alternative: Standalone PostgreSQL

Users who don't want to clone AIDA can run PostgreSQL however they like and just pass the connection string:

```bash
aida --file "postgres://user:pass@localhost:5432/mydb" list
```

## Phase 1: Development Workflow (`make dev`)

**Goal**: One command to start developing AIDA with hot-reload.

### New Files

| File | Purpose |
|------|---------|
| `.aida/docker-compose.dev.yml` | PostgreSQL only (no aida-server) |
| `Makefile` additions | `dev`, `dev-pg`, `dev-stop` targets |

### `.aida/docker-compose.dev.yml`

```yaml
# Development: PostgreSQL only — code runs natively
services:
  db:
    image: postgres:15-alpine
    ports:
      - "${AIDA_PG_PORT:-5432}:5432"
    environment:
      POSTGRES_USER: ${AIDA_PG_USER:-aida}
      POSTGRES_PASSWORD: ${AIDA_PG_PASSWORD:-aida}
      POSTGRES_DB: aida_default
    volumes:
      - ./pgdata:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U aida"]
      interval: 5s
      timeout: 5s
      retries: 5
```

### Makefile Targets

```makefile
# Development (hot-reload)
dev:              ## Start full dev environment (PostgreSQL + cargo-watch + Vite)
dev-server:       ## Start only Rust server with cargo-watch
dev-web:          ## Start only Vite dev server
dev-pg:           ## Start only PostgreSQL in Docker
dev-stop:         ## Stop all dev services
```

`make dev` orchestrates:
1. Checks if PostgreSQL is running, starts it if not (`docker compose -f .aida/docker-compose.dev.yml up -d`)
2. Waits for PG to be ready (`pg_isready`)
3. Migrates existing requirements.db → PostgreSQL if needed
4. Launches `cargo watch -x 'run -p aida-server -- --rest-port 8080 --database postgres://aida:aida@localhost:5432/aida_default --static-dir aida-web-react/dist'` in background
5. Launches `cd aida-web-react && npm run dev` in background
6. Prints URLs: API at 8080, Web at 5173

`make dev-stop`:
1. Kills cargo-watch and Vite processes
2. Optionally stops PostgreSQL (`docker compose -f .aida/docker-compose.dev.yml down`)

### Cargo Watch

Requires `cargo-watch` (`cargo install cargo-watch`). Watches `aida-core/` and `aida-server/` for changes, rebuilds incrementally (~5s).

### Build Consideration

The main `Dockerfile` currently doesn't compile with `--features postgres`. For Phase 1 (dev only), native builds already have it. For Phase 2, the Dockerfile needs updating.

## Phase 2: End-User Server Management (`aida server start/stop`)

**Goal**: Any user can start/stop the AIDA server + database from the CLI.

### New CLI Subcommands

```bash
aida server start              # Start PostgreSQL + web dashboard
aida server stop               # Stop everything
aida server status             # Show running services, ports, connected projects
aida server logs               # Tail server logs
aida server auth <mode>        # Set auth mode: none | pin | oidc
aida server db create <name>   # Create a new project database
aida server db list            # List all project databases
```

### How `aida server start` Works

1. Locates the AIDA installation directory (where docker-compose lives)
   - Check `AIDA_HOME` env var
   - Check `~/.aida/home` for a path pointing to the install dir
   - Fall back to the directory containing the `aida` binary
2. Checks ports 8080 and 5432 — fail with clear message if in use
3. Registers ports in `~/.ports`
4. On first run: generates a PIN, stores in `.aida/server.toml`, prints it
5. Runs `docker compose -f <aida-home>/.aida/docker-compose.yml up -d`
6. Waits for services to be healthy
7. Prints: `AIDA server running at http://localhost:8080 (PIN: 7294)`

### Updated `.aida/docker-compose.yml`

```yaml
services:
  db:
    image: postgres:15-alpine
    ports:
      - "${AIDA_PG_PORT:-5432}:5432"
    environment:
      POSTGRES_USER: ${AIDA_PG_USER:-aida}
      POSTGRES_PASSWORD: ${AIDA_PG_PASSWORD:-aida}
      POSTGRES_DB: aida_default
    volumes:
      - ./pgdata:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U aida"]
      interval: 5s
      timeout: 5s
      retries: 5

  server:
    build:
      context: ..
      dockerfile: Dockerfile
    ports:
      - "${AIDA_PORT:-8080}:8080"
    depends_on:
      db:
        condition: service_healthy
    environment:
      AIDA_DATABASE_URL: postgres://${AIDA_PG_USER:-aida}:${AIDA_PG_PASSWORD:-aida}@db:5432/aida_default
      RUST_LOG: info
    env_file:
      - ../.env
```

### Scaffolded Project Connection

When `aida init` runs in a new project:
1. Checks if an AIDA server is running (ping `localhost:8080`)
2. If yes: creates a database via `aida server db create <project-name>`
3. Writes `.aida/config.toml` with the connection string
4. CLI commands automatically use this connection
5. If no server: scaffolds YAML mode, prints hint about `aida server start`

```toml
# .aida/config.toml
[server]
url = "postgres://aida:aida@localhost:5432/aida_myproject"
web = "http://localhost:8080"
```

## Phase 3: Deprecate SQLite Default

1. `aida init` defaults to YAML (no infrastructure) when no server is running
2. `aida init` auto-connects to PostgreSQL when server is detected
3. SQLite remains as `aida --file foo.db` for migration/one-off use
4. Update CLAUDE.md, docs, README to reflect YAML + PostgreSQL as the two paths
5. Pre-commit hook continues to work for YAML-only projects

## Migration Path

```bash
# Existing SQLite project → PostgreSQL
aida server start
aida db migrate --from sqlite --to postgres --output "postgres://aida:aida@localhost:5432/aida_myproject"

# Existing SQLite project → YAML (simple fallback)
aida db migrate --from sqlite --to yaml --output requirements.yaml
```

## Open Questions

1. **Remote server**: Should `aida server start` support running on a remote host for team use, or is that a separate deployment concern? (The existing `docker/docker-compose.yml` with Traefik already covers this.)

## Related Requirements

- To be created during Phase 1 implementation

## Status

Approved — starting Phase 1
