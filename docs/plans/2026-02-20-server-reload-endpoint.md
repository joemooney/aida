# Server Reload Endpoint + Automatic mtime-based Reload

## Context

The aida-server loads the SQLite database into an in-memory `RwLock<RequirementsStore>` at startup. When the CLI (`aida add`, `aida edit`) writes directly to SQLite, the server's in-memory state becomes stale. This plan adds:
1. An explicit `POST /api/v2/reload` endpoint
2. Automatic lazy reload on reads when the DB file's mtime has changed

## Files Modified

| File | Change |
|------|--------|
| `aida-server/src/service.rs` | Added `last_loaded_mtime` field to `ServerState`, added `reload()` and `check_reload()` methods |
| `aida-server/src/rest.rs` | Added `POST /api/v2/reload` route + handler, added `check_reload()` in V2 read endpoints |

## Implementation Details

### Phase 1: ServerState Changes (`service.rs`)
- Added `last_loaded_mtime: RwLock<SystemTime>` field to `ServerState`
- In `new()`: initialized from `fs::metadata(backend.path()).modified()` with fallback to `UNIX_EPOCH`
- `reload()`: calls `backend.load()`, replaces store contents, updates mtime, returns requirement count
- `check_reload()`: compares current file mtime with last loaded mtime, triggers reload if newer, silently ignores errors

### Phase 2: REST Endpoints (`rest.rs`)
- Added `POST /api/v2/reload` route returning `{ "reloaded": true, "requirements": count }`
- Added `check_reload()` calls in `list_requirements_v2_legacy`, `get_requirement_v2_legacy`, and `search_requirements_v2_legacy`

## Related Requirements
- FR-0227 (Server REST API)

## Status
Completed
