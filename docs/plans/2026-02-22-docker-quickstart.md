# Docker Quickstart for AIDA

## Summary

Add Docker support as the recommended quickstart path. A single `docker compose up` gives a fully working AIDA instance (REST API + React dashboard on port 8080) backed by SQLite, with zero native dependencies.

## Approach

- **Static file serving**: `--static-dir` flag on aida-server uses tower-http `ServeDir` with SPA fallback to serve the React dashboard from the same port as the API
- **3-stage Dockerfile**: Node frontend build → Rust binary build → slim Debian runtime
- **Single-container**: Both API and dashboard served from port 8080
- **Cargo.lock committed**: Removed from .gitignore (Rust best practice for applications)

## Files Changed

| File | Action |
|------|--------|
| `Cargo.toml` | Added `"fs"` to tower-http features |
| `aida-server/src/main.rs` | Added `--static-dir` arg + `fallback_service` in both router branches |
| `.gitignore` | Removed `Cargo.lock` |
| `Dockerfile` | Created — 3-stage multi-stage build |
| `docker-compose.yml` | Created — quickstart single-service compose |
| `.dockerignore` | Created |
| `OVERVIEW.md` | Docker-first Getting Started section |
| `Makefile` | Added docker-build/up/down/shell targets |
| `CLAUDE.md` | Added Docker quickstart mention |

## Related Requirements

- Docker deployment support

## Status
Completed
