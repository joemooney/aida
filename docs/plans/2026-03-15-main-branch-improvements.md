# Main Branch Improvements Plan

**Date**: 2026-03-15
**Status**: In Progress
**Branch**: main (parallel to `distributed-architecture` branch)

## Context

While the distributed architecture is explored on a separate branch, the main branch continues with high-impact improvements to AIDA's current centralized architecture. These items are drawn from the strategic evaluation of the distributed spec and existing roadmap priorities.

## Phase 1: Foundation Improvements (Current Sprint)

### 1.1 UUID v7 as Canonical Machine Identity
- Add UUID v7 column to SQLite and PostgreSQL schemas
- Generate UUID v7 on requirement creation (alongside existing spec_id)
- Use UUID v7 for cross-system references and future sync scenarios
- Dependency: `uuid` crate already included; switch from v4 to v7 generation
- **Why**: Independently valuable regardless of distributed architecture outcome. Time-ordered UUIDs improve index locality and enable future deduplication.

### 1.2 Immutable ID Principle (Documentation)
- Document in CLAUDE.md that spec_ids are immutable once committed
- Add validation to prevent ID reassignment in the API
- Ensure `aida edit` cannot change a requirement's spec_id
- **Why**: Correctness principle that applies to both centralized and distributed modes.

### 1.3 HLC Timestamps (Library Addition)
- Add Hybrid Logical Clock implementation as a library module in aida-core
- Use HLC for `modified_at` timestamps in new writes
- Backward-compatible: existing wall-clock timestamps remain valid
- **Why**: Better ordering semantics for multi-user PostgreSQL deployments. Required foundation if distributed mode is adopted later.

### 1.4 Field-Level Conflict Detection Enhancement
- Extend existing `FieldConflict` in storage.rs to detect per-field conflicts on concurrent edits
- Surface conflicts in the REST API response (not just version mismatch errors)
- Add conflict resolution UI in React dashboard (accept-mine / accept-theirs)
- **Why**: Already partially implemented. Completes the multi-user story for centralized PostgreSQL.

## Phase 2: PostgreSQL-First Completion

### 2.1 Finish `aida server start/stop` (from 2026-03-08 plan)
- Implement CLI subcommands: `aida server start`, `stop`, `status`, `logs`
- Auto-generate PIN on first run
- Docker compose for PostgreSQL + aida-server
- Port registration in `~/.ports`

### 2.2 Project Scaffolding with Server Detection
- `aida init` detects running server, creates database, writes connection config
- Falls back to YAML mode when no server is running
- Smooth onboarding: `aida init` → working project in 30 seconds

### 2.3 SQLite Deprecation Path
- Default to YAML (no server) or PostgreSQL (server detected)
- SQLite remains as `aida --file foo.db` for migration/one-off use
- Update documentation to reflect YAML + PostgreSQL as the two paths

## Phase 3: Ecosystem Integration

### 3.1 GitHub Integration
- Bidirectional sync with GitHub Issues (similar to existing GitLab integration)
- Map AIDA requirement types to GitHub labels
- Sync status changes, comments, and relationships
- **Why**: Biggest ecosystem gap identified in WHY-AIDA.md

### 3.2 API Key Authentication
- Simple API key auth for multi-user without OIDC overhead
- Generate keys via `aida server auth apikey --create`
- Store hashed keys in server config
- **Why**: Enables team use without enterprise OIDC setup

### 3.3 OIDC Authentication
- Full OIDC flow for enterprise deployments
- Already partially implemented in web_auth.rs
- Complete the flow with token refresh, session management

## Phase 4: User Experience

### 4.1 Onboarding Polish
- Improve `/aida-onboard` skill with guided setup
- "Try in 30 seconds" Docker quick-start
- First-run tutorial in React dashboard

### 4.2 Real-Time Collaboration (SSE Enhancement)
- Extend existing SSE infrastructure for multi-user presence
- Show "who is viewing/editing" indicators in the React dashboard
- Sync freshness indicator (live/stale/offline) in the UI footer
- **Why**: Cherry-picked from distributed spec Section 9.7 — valuable for centralized mode too.

### 4.3 Non-Claude AI Support
- Cursor rules generation from AIDA requirements
- Generic MCP client support
- Windsurf configuration generation

## Phase 5: Analytics & Reporting

### 5.1 Velocity and Trend Analytics
- Sprint velocity charts (planned vs completed)
- Requirement churn metrics
- AI contribution tracking (trace comment analysis)

### 5.2 Quality Score Dashboard
- Aggregate AI evaluation scores across requirements
- Trend visualization over time
- Quality gates for sprint completion

## Cherry-Picked Ideas from Distributed Spec

These ideas from the distributed architecture evaluation are independently valuable and should be folded into the main branch:

| Idea | Source | Priority | Notes |
|---|---|---|---|
| UUID v7 canonical identity | Spec Section 2.3 | Phase 1 | Replace v4 with v7 for time-ordering benefits |
| Immutable ID principle | Spec Section 2, G-01 | Phase 1 | Document and enforce as a constraint |
| HLC timestamps | Spec Section 10.3 | Phase 1 | Library addition, backward-compatible |
| SSE real-time updates enhancement | Spec Section 9.2-9.7 | Phase 4 | Extend existing SSE for presence and freshness |
| Sync freshness indicator | Spec Section 9.7 | Phase 4 | UI component showing data staleness |
| Append-only relations with tombstones | Spec Section 7.4, G-11 | Phase 2 | Better audit trail for relationship changes |
| One-file-per-object export format | Spec Section 7.2, G-09 | Phase 3 | Optional TOML export for git-diffable snapshots |

## Relationship to Distributed Branch

- Main branch: centralized PostgreSQL-first architecture
- Distributed branch: git-as-event-log with node-namespaced IDs
- Shared foundations: UUID v7, HLC timestamps, field-level conflict detection
- Goal: both branches converge on a dual-mode architecture where deployment mode is configurable

## Related Requirements

- FR-0316: PostgreSQL backend (completed)
- STORY-0321-0327: GitLab integration (completed)
- EPIC-0365: Personal queue (completed)
- New requirements to be created as phases begin

## Open Questions

1. Should HLC timestamps replace wall-clock timestamps everywhere, or only for new writes?
2. How should the UUID v7 migration handle existing requirements that have v4 UUIDs (or no UUIDs)?
3. What's the minimum viable GitHub integration — unidirectional push, or full bidirectional sync?
