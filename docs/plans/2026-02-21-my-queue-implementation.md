# My Queue: Full-Stack Implementation Plan (EPIC-0365)

## Related Requirements
- EPIC-0365: My Queue epic
- STORY-0366: Database Storage
- STORY-0367: REST API Endpoints
- STORY-0368: CLI Commands
- STORY-0369: React Web UI
- STORY-0370: Dashboard Widget
- STORY-0371: Assign-to-Queue Inbox

## Status
Completed

## Summary

Implemented "My Queue" as a personal focus inbox — an ordered list of requirements each user wants to work on. Queue is stored as a separate SQL table (`queue_entries`), not part of the RequirementsStore, keeping it decoupled from the requirement model.

### Phase 1: Database Storage
- Added `QueueEntry` model to `aida-core/src/models.rs`
- Added 5 queue trait methods to `DatabaseBackend` trait (with default "not supported" errors for YAML)
- SQLite schema migration v6→v7 with `queue_entries` table
- PostgreSQL schema migration v6→v7 with `queue_entries` table
- Implemented all 5 methods for both SQLite and PostgreSQL backends
- Added `Storage` wrapper methods for CLI access

### Phase 2: REST API
- Added 5 queue endpoints to `aida-server/src/rest.rs`:
  - `GET /api/v2/queue/:user_id` — list queue entries
  - `POST /api/v2/queue/:user_id` — add to queue
  - `DELETE /api/v2/queue/:user_id/:req_id` — remove from queue
  - `PATCH /api/v2/queue/:user_id/:req_id` — update position/note
  - `POST /api/v2/queue/:user_id/reorder` — bulk reorder

### Phase 3: CLI Commands
- Added `QueueCommand` enum with List, Add, Remove, Move, Clear subcommands
- Supports `--user`, `--top`, `--bottom`, `--before`, `--note` flags
- User defaults to AIDA_USER env → USER env → "default"

### Phase 4: React Web UI
- Added `QueueEntry` type to `shared/types.ts`
- Created `aida-web-react/src/api/queue.ts` — API client
- Created `aida-web-react/src/hooks/useQueue.ts` — React Query hooks with optimistic updates
- Created `QueuePage` with drag-to-reorder via @dnd-kit/sortable
- Created `QueueItem` with drag handle, remove button, badges
- Added `/queue` route and "My Queue" nav item with Inbox icon
- Added "Add to Queue" button in DetailHeader and RequirementsRow (hover)

### Phase 5: Dashboard Widget
- Created `QueueWidget` showing top 5 items with "View All →" link
- Hidden when queue is empty
- Added to DashboardPage after SprintSummary

### Phase 6: Assign-to-Queue
- Handled by design: `added_by` field, `note` field, CLI `--user` flag
