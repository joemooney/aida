# My Queue: Personal Focus Inbox — Implementation Plan

## Related Requirements
- **EPIC-0365**: My Queue: Personal Focus Inbox
- **STORY-0366**: Queue: Database storage model
- **STORY-0367**: Queue: REST API endpoints
- **STORY-0368**: Queue: CLI commands
- **STORY-0369**: Queue: React web UI — My Queue view
- **STORY-0370**: Queue: Dashboard focus widget
- **STORY-0371**: Queue: Assign-to-queue (inbox) capability
- **FR-0189**: Work Queue View (existing, GUI — superseded by this plan)
- **FR-0340**: Team Queue View (existing, GUI — incorporated)
- **FR-0313**: My Queue should filter out complete by default (incorporated)

## Status
Not Started

---

## Concept

My Queue is a **personal, ordered focus list** — orthogonal to sprints, priorities, and status:

| Dimension | What it answers |
|-----------|----------------|
| Sprint | What did the team commit to this iteration? |
| Priority | How important is this globally? |
| Owner | Who is responsible for this? |
| Status | Where is this in the workflow? |
| **My Queue** | **What am I working on right now, in what order?** |

A user might own 30 items but have 5 in their queue. Others can push items in (inbox semantics), but only the queue owner controls ordering and removal.

---

## Phase 1: Database Storage Model (STORY-0366)

### Schema

```sql
CREATE TABLE queue_entries (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id     TEXT NOT NULL,           -- user handle (e.g., "joe")
    requirement_id TEXT NOT NULL,        -- requirement UUID
    position    INTEGER NOT NULL,        -- sort order (gapped: 1000, 2000, ...)
    added_by    TEXT NOT NULL,           -- who added it (self or another user)
    note        TEXT,                    -- optional context ("urgent, client X")
    added_at    TEXT NOT NULL,           -- ISO 8601 timestamp
    UNIQUE(user_id, requirement_id)
);
CREATE INDEX idx_queue_user_position ON queue_entries(user_id, position);
```

### Position Strategy
- New items: append at `max(position) + 1000` (or insert at specific gap)
- Reorder: swap positions or reassign
- Re-normalize: when gaps shrink below 10, redistribute evenly across 1..N*1000
- Top insert: `min(position) - 1000` (or 500 if near zero, then renormalize)

### Files to Modify

| File | Change |
|------|--------|
| `aida-core/src/storage/sqlite.rs` | Add `queue_entries` table creation in migrations |
| `aida-core/src/storage/sqlite.rs` | Add CRUD methods: `queue_list`, `queue_add`, `queue_remove`, `queue_reorder`, `queue_move` |
| `aida-core/src/storage/postgres.rs` | Same schema and methods for PostgreSQL backend |
| `aida-core/src/storage/mod.rs` | Add queue trait methods to `Storage` trait |
| `aida-core/src/models.rs` | Add `QueueEntry` struct |

### Data Model (Rust)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueEntry {
    pub id: i64,
    pub user_id: String,
    pub requirement_id: Uuid,
    pub position: i64,
    pub added_by: String,
    pub note: Option<String>,
    pub added_at: DateTime<Utc>,
}
```

### Cascade Behavior
- When a requirement is deleted → remove from all queues
- When a requirement status changes to Completed → retain in queue but filtered by default

---

## Phase 2: REST API Endpoints (STORY-0367)

### Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v2/queue/:user_id` | List queue (sorted by position) |
| `POST` | `/api/v2/queue/:user_id` | Add item `{ requirement_id, position?, note? }` |
| `DELETE` | `/api/v2/queue/:user_id/:req_id` | Remove item |
| `PATCH` | `/api/v2/queue/:user_id/:req_id` | Update position or note |
| `POST` | `/api/v2/queue/:user_id/reorder` | Bulk reorder `{ items: [{id, position}] }` |
| `GET` | `/api/v2/queue/:user_id/summary` | Top N items + count (for dashboard widget) |

### Query Parameters
- `?include_completed=true` — include completed items (default: false)
- `?limit=N` — limit results

### Response Shape
```json
{
  "entries": [
    {
      "position": 1000,
      "requirement_id": "uuid",
      "spec_id": "FR-0042",
      "title": "Implement login validation",
      "status": "InProgress",
      "priority": "High",
      "added_by": "manager",
      "note": "urgent, client escalation",
      "added_at": "2026-02-20T10:00:00Z"
    }
  ],
  "total": 12
}
```

Entries are enriched with requirement summary fields to avoid N+1 lookups on the client.

### Files to Modify

| File | Change |
|------|--------|
| `aida-server/src/routes/` | New `queue.rs` module with handlers |
| `aida-server/src/main.rs` | Register `/api/v2/queue/` routes |
| `shared/types.ts` | Add `QueueEntry` TypeScript type |

---

## Phase 3: CLI Commands (STORY-0368)

### Commands

```
aida queue list [--user <handle>]
aida queue add <SPEC-ID> [--top|--bottom|--after <SPEC-ID>] [--user <handle>] [--note "..."]
aida queue remove <SPEC-ID> [--user <handle>]
aida queue move <SPEC-ID> --top|--bottom|--before <SPEC-ID>|--after <SPEC-ID>
aida queue clear [--user <handle>] [--completed]
```

### Display Format
```
  #  SPEC-ID     Title                           Status       Priority   Added By
  1  FR-0042     Implement login validation       In Progress  High       @self
  2  BUG-0023    Fix null response handling        Draft        Medium     @manager  "urgent"
  3  STORY-0366  Queue: Database storage model     Approved     High       @self
```

### Files to Modify

| File | Change |
|------|--------|
| `aida-cli/src/main.rs` or `commands/` | Add `queue` subcommand with sub-subcommands |
| `aida-core/src/` | Queue business logic (may already exist from Phase 1) |

### User Resolution
- Default user: `AIDA_USER` env var → git config `user.name` → system username
- `--user` flag overrides for cross-user operations

---

## Phase 4: React Web UI — My Queue View (STORY-0369)

### New Route: `/queue`

### Components

| Component | File | Description |
|-----------|------|-------------|
| `QueuePage` | `src/components/queue/QueuePage.tsx` | Main page with header, queue list, empty state |
| `QueueItem` | `src/components/queue/QueueItem.tsx` | Single queue entry row (draggable) |
| `AddToQueueButton` | `src/components/queue/AddToQueueButton.tsx` | Reusable action button for List/Board/Detail |

### Sidebar Navigation
- New entry: icon `Inbox` from lucide-react, label "My Queue", route `/queue`
- Position: after Dashboard, before Kanban Board

### Queue List Features
- Drag-and-drop reorder (using native HTML5 drag or a lightweight lib)
- Each row: position number, spec-id, title, status pill, priority dot, added-by badge
- Items added by others: subtle highlight or "inbox" badge
- Click row → detail panel slides in (same pattern as List View)
- "Clear Completed" button in header
- Toggle: "Show Completed" (off by default per FR-0313)

### Adding to Queue from Other Views
- List View: right-click context menu or action icon on row → "Add to Queue"
- Board View: card action menu → "Add to Queue"
- Detail Panel: action button → "Add to Queue"

### Hooks

| Hook | File | Description |
|------|------|-------------|
| `useQueue` | `src/hooks/useQueue.ts` | Fetch/mutate queue via react-query |

### Files to Create

| File | Description |
|------|-------------|
| `src/components/queue/QueuePage.tsx` | Page component |
| `src/components/queue/QueueItem.tsx` | Draggable row component |
| `src/components/queue/AddToQueueButton.tsx` | Reusable add-to-queue action |
| `src/hooks/useQueue.ts` | react-query hook for queue API |

### Files to Modify

| File | Change |
|------|--------|
| `src/components/layout/Sidebar.tsx` | Add "My Queue" nav item |
| `src/App.tsx` | Add `/queue` route |
| `src/components/list/RequirementsRow.tsx` | Add "Add to Queue" context action |
| `src/components/detail/DetailHeader.tsx` | Add "Add to Queue" action button |

---

## Phase 5: Dashboard Focus Widget (STORY-0370)

### Widget: "My Focus"
- Compact card on Dashboard, placed between SprintSummary and charts
- Shows top 3-5 queue items (spec-id, title, status pill)
- Total count badge
- "View All →" link to `/queue`
- Hidden when queue is empty

### Files to Create

| File | Description |
|------|-------------|
| `src/components/dashboard/QueueWidget.tsx` | Dashboard focus widget |

### Files to Modify

| File | Change |
|------|--------|
| `src/components/dashboard/DashboardPage.tsx` | Add `<QueueWidget>` after SprintSummary |

---

## Phase 6: Assign-to-Queue / Inbox (STORY-0371)

### UI: "Assign to Queue" Action
- Available in detail panel and context menus
- Opens user picker → optional note input → confirms
- Uses `POST /api/v2/queue/:target_user` with `added_by` set to current user

### Visual Distinction
- Queue items where `added_by !== current_user` show:
  - "Added by @name" badge
  - Note text (if present) in muted italic
  - Slightly different background (e.g., left border accent)

### CLI
- `aida queue add FR-042 --user joe --note "please review"` — adds to joe's queue, `added_by` = current user

---

## Implementation Order

```
Phase 1 (Database)  ──→  Phase 2 (API)  ──→  Phase 3 (CLI)
                                         ──→  Phase 4 (Web UI)  ──→  Phase 5 (Widget)
                                                                 ──→  Phase 6 (Inbox)
```

**Recommended sequence:**
1. **Phase 1**: Database model — foundation for everything
2. **Phase 2**: REST API — enables both CLI and web
3. **Phase 3 + 4** (parallel): CLI commands + React UI
4. **Phase 5**: Dashboard widget — quick add once Phase 4 exists
5. **Phase 6**: Inbox/assign — enhancement once core queue works

---

## Migration from GUI Local Queue

The existing aida-gui stores queue in `~/.config/aida/aida_gui_settings.yaml`. After Phase 1:
- Add a one-time migration in aida-gui that reads local queue entries and writes them to the database via the new storage methods
- Remove local queue from `UserSettings` struct after migration period
- GUI switches to using database-backed queue operations

---

## Design Principles

1. **Lightweight**: No workflows, no approval gates. Just an ordered list.
2. **Inbox metaphor**: Others can push in, only owner controls order.
3. **Ephemeral**: Items flow in and out. Not a permanent record.
4. **Orthogonal**: Doesn't replace priority, status, sprint, or owner. Complements them.
5. **Auto-tidy**: Completed items filter out automatically. No manual cleanup needed.
