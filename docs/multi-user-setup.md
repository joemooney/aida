# AIDA Multi-User Setup Guide

**Last updated**: 2026-06-06

> **Most multi-user teams want the default git-canonical store, not this guide.** A plain `aida init` gives you a shared multi-user requirement store (the orphan `aida-store` branch on your remote, with a local read cache per clone) that already gives you offline capability and git-native conflict resolution — no server to run, no database to administer. Reach for the **PostgreSQL** path documented here only when you specifically want an always-online server-of-record with SQL-level access patterns plus a REST API and React dashboard. PostgreSQL is an advanced, opt-in deployment — not the getting-started flow.

This guide covers setting up AIDA for multi-user access via **PostgreSQL** — a server-backed shared projection where all users connect to a single PostgreSQL database.

> **PostgreSQL support is opt-in via the `postgres` Cargo feature.** A default `cargo install aida-cli` will NOT include PostgreSQL drivers. Build with `--features postgres` to enable. A binary built without it cannot open a `postgres://` store.

---

## Prerequisites

- PostgreSQL 15+ running (via Docker or native)
- `aida` and `aida-server` binaries built with PostgreSQL support: `cargo build --features postgres` (add `,gitlab,github,jira` if you also want those integrations — all opt-in)
- Network access between machines (same LAN or VPN)

## Quick Start

### 1. Start PostgreSQL

```bash
# Using the dev docker-compose (simplest)
make dev-pg

# Or start manually
docker compose -f .aida/docker-compose.dev.yml up -d
```

This starts PostgreSQL on port 5432 with:
- User: `aida`
- Password: `aida`
- Database: `aida_default`

### 2. Seed the PostgreSQL store

A fresh PostgreSQL database starts empty. You have two ways to populate it:

- **Start fresh** — just point the server (next step) at the new database and add requirements directly via the CLI / REST API / dashboard. This is the common case.
- **Import from a legacy store** — only if you have an existing *pre-git-canonical* store (a standalone `requirements.db` SQLite file or a `requirements.yaml`, i.e. a project created with `aida init --centralized`). The one-shot `aida db migrate` helper is kept around for exactly this:

  ```bash
  aida db migrate --from sqlite --to postgres \
    --output "postgres://aida:aida@localhost:5432/aida_default"
  # --from yaml works the same way for a legacy YAML store
  ```

  > `aida db migrate` is an **archived legacy command** — it exists only to lift data off the deprecated centralized backends. The default git-canonical store (orphan `aida-store` branch) does not use it. To move a git-canonical store into PostgreSQL, see [Switching Between Modes](#switching-between-modes) below.

### 3. Verify the Data

```bash
aida --file "postgres://aida:aida@localhost:5432/aida_default" list
```

### 4. Start the Server

```bash
# Using make (recommended)
make serve

# Or manually
aida-server \
  --host 0.0.0.0 \
  --rest-port 8080 \
  --database "postgres://aida:aida@localhost:5432/aida_default"
```

The server binds to `0.0.0.0` so it's accessible from other machines.

---

## Connecting from Other Machines

Find the server's IP address:

```bash
hostname -I | awk '{print $1}'
# Example: 192.168.179.180
```

### Option A: Direct CLI Access

Install the `aida` binary (built `--features postgres`) on the other machine, then pass the connection string with `--file` on each call:

```bash
aida --file "postgres://aida:aida@192.168.179.180:5432/aida_default" list
aida --file "postgres://aida:aida@192.168.179.180:5432/aida_default" add \
  --title "From Alice's machine" --type story --status draft
aida --file "postgres://aida:aida@192.168.179.180:5432/aida_default" show FR-0042
```

> The `postgres://` connection string is passed only via the `--file` flag — there is no `AIDA_FILE` environment variable. To avoid retyping it, define a shell alias, e.g. `alias aida-pg='aida --file "postgres://aida:aida@192.168.179.180:5432/aida_default"'`.

### Option B: REST API

No `aida` binary needed — any HTTP client works:

```bash
# List all requirements
curl http://192.168.179.180:8080/api/v2/requirements | python3 -m json.tool

# Get a single requirement
curl http://192.168.179.180:8080/api/v2/requirements/FR-0042

# Create a requirement
curl -X POST http://192.168.179.180:8080/api/v2/requirements \
  -H "Content-Type: application/json" \
  -d '{"title": "New from API", "description": "Created remotely", "req_type": "functional"}'

# Update a requirement
curl -X PUT http://192.168.179.180:8080/api/v2/requirements/FR-0042 \
  -H "Content-Type: application/json" \
  -d '{"status": "approved", "owner": "alice"}'

# Search
curl "http://192.168.179.180:8080/api/v2/search?q=authentication"
```

### Option C: React Dashboard

Open in any browser on the network:

```
http://192.168.179.180:8080
```

The dashboard connects to the REST API automatically.

### Option D: MCP Integration (Claude Code)

On any machine with Claude Code, add to `.mcp.json`:

```json
{
  "mcpServers": {
    "aida": {
      "command": "aida",
      "args": ["--file", "postgres://aida:aida@192.168.179.180:5432/aida_default", "mcp-serve"]
    }
  }
}
```

---

## Concurrency & Conflict Handling

PostgreSQL handles multi-user access natively:

- **Multiple readers**: unlimited concurrent reads, no locking
- **Multiple writers**: PostgreSQL's MVCC handles concurrent writes
- **Optimistic locking**: each requirement has a `version` field — if two users edit the same requirement simultaneously, the second save gets a version conflict error with a clear message
- **No data loss**: conflicts are reported, never silently overwritten

### What Happens on Conflict

```
User A loads FR-0042 (version 5)
User B loads FR-0042 (version 5)
User A saves changes → succeeds (version becomes 6)
User B saves changes → error: "Version conflict for FR-0042: expected 5, current 6"
User B reloads, merges changes, saves → succeeds (version 7)
```

The REST API returns HTTP 409 (Conflict) with the conflict details.

---

## Production Deployment

For internet-facing deployment with TLS, use `docker/docker-compose.yml` which includes:

- **Traefik**: reverse proxy with automatic Let's Encrypt TLS via Cloudflare DNS
- **PostgreSQL**: production instance with secret-based credentials
- **aida-server**: REST API + gRPC at `aida-api.joemooney.com`
- **aida-web**: React dashboard at `aida.joemooney.com`
- **Cloudflare tunnel**: secure access without opening ports

```bash
cd docker
docker compose up -d
```

### Production Secrets

Stored in `docker/secrets/` (gitignored):

| File | Purpose |
|------|---------|
| `pg_user.txt` | PostgreSQL username |
| `pg_password.txt` | PostgreSQL password |
| `aida_database_url.txt` | Connection string for aida-server |
| `pgadmin_password.txt` | pgAdmin web interface password |
| `cloudflare_api_token.txt` | Cloudflare API token for DNS TLS |

### Authentication

For production multi-user, configure authentication:

```bash
# PIN-based (simple)
aida-server --auth-mode pin --auth-pin 7294

# API key
aida-server --auth-mode apikey

# OIDC (enterprise)
aida-server --auth-mode oidc --oidc-issuer https://auth.example.com
```

See `aida-server --help` for all auth options.

---

## Switching Between Modes

Both modes use the same requirement format, so migration is lossless. The two helpers below — `db export-git` and `db migrate` — are **archived legacy commands** kept for one-shot moves on and off the centralized backend; they are not part of day-to-day operation.

### PostgreSQL → git-canonical (the default)

If you later want offline/disconnected operation and the git-native store:

```bash
# Export the PostgreSQL store to a git-backed store directory
aida --file "postgres://aida:aida@localhost:5432/aida_default" db export-git -o aida-store

# Then initialize the default distributed mode (or attach the exported store)
aida init
```

### git-canonical → PostgreSQL

```bash
# Import a git-backed (YAML) store into PostgreSQL
aida db migrate --from yaml --to postgres \
  --output "postgres://aida:pass@host:5432/aida_default"
```

---

## Troubleshooting

### Can't connect from another machine

```bash
# Check PostgreSQL is listening on all interfaces
ss -tlnp | grep 5432
# Should show *:5432, not 127.0.0.1:5432

# Check firewall
sudo ufw status  # Ubuntu
sudo firewall-cmd --list-ports  # RHEL/Fedora

# Test connectivity from the other machine
pg_isready -h 192.168.179.180 -p 5432 -U aida
```

### Version conflict errors

This means another user modified the same requirement. Reload and retry:

```bash
aida --file "postgres://..." show FR-0042  # get latest version
aida --file "postgres://..." edit FR-0042 --status approved  # retry
```

### PostgreSQL not starting

```bash
# Check container logs
docker compose -f .aida/docker-compose.dev.yml logs db

# Check if port is already in use
ss -tlnp | grep 5432
```

### Slow queries with many requirements

PostgreSQL handles 100K+ requirements without issues. If queries are slow:

```bash
# Check if indexes exist
docker exec aida_db_1 psql -U aida -d aida_default \
  -c "SELECT indexname FROM pg_indexes WHERE tablename = 'requirements';"
```

---

## Architecture

```
                    ┌─────────────────────────┐
                    │     PostgreSQL 15        │
                    │   aida_default database  │
                    │   354+ requirements      │
                    │   Optimistic locking     │
                    └────────┬────────────────┘
                             │
              ┌──────────────┼──────────────┐
              │              │              │
         ┌────┴────┐   ┌────┴────┐   ┌─────┴────┐
         │  aida   │   │  aida   │   │  aida    │
         │  CLI    │   │ server  │   │  CLI     │
         │ (Joe)   │   │ REST+gRPC   │ (Alice)  │
         └─────────┘   └────┬────┘   └──────────┘
                            │
                     ┌──────┴──────┐
                     │   React     │
                     │  Dashboard  │
                     │  (browser)  │
                     └─────────────┘
```

All clients connect to the same PostgreSQL instance. The server is optional — CLI can connect directly to PostgreSQL. The server adds the REST API and React dashboard.
