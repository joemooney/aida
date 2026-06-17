# Team dashboard (aida-server + aida-web-react)

- **Date:** 2026-06-17
- **Specs:** EPIC-47 Tier 3 (the dashboard item). Builds on assignment (STORY-639), roster+coordination view (STORY-640), RBAC (STORY-646/647), coordination (EPIC-46).
- **Status:** Design + slices. Grounded in the 2026-06-16 server/web exploration.
- **Gate:** server endpoint tests + the web app builds/typechecks; the dashboard reads existing substrate so the multi-clone harness already covers the underlying data.

## 0. What it is

A web view of the team's state on the existing stack: **aida-server** (axum REST :8080 + tonic gRPC) + **aida-web-react** (Vite + React 19 + React Query + Tailwind + dnd-kit, already has a `DashboardPage` + `MetricsCards`/`StatusChart`/`SprintSummary`/`QueueWidget`). It surfaces the team capabilities we just built — roster + roles, who's assigned what, who holds what (coordination claims), and burndown — so a team lead sees the whole picture in a browser instead of N CLI calls.

## 1. Reuse map (from the exploration)

Already there, reuse directly:
- `GET /api/v2/requirements` returns full `Requirement[]` incl. `assignee` (STORY-639) → **assignment board** needs no new endpoint (filter/group client-side).
- `GET /api/v2/users` → `User[]`; `GET /api/v2/queue/:user` → per-user queue; `GET /api/v2/analytics` → status/velocity → **burndown** reuses it.
- React Query hooks pattern (`useRequirements`/`useQueue`), `apiFetch<T>()` client, ts-rs → `shared/types.ts`, React Router v7 + `Sidebar` (add a nav item), `DashboardPage` component patterns (`StatusChart` donut, card layout).

The gaps (new work):
- **No `/team` endpoint** — roster (nodes.toml) + per-user roles (team.toml, STORY-646) aren't REST-exposed.
- **No `/coordination` endpoint** — active leases + drain/solo claims (`coordination/`, EPIC-46) aren't REST-exposed.

## 2. Decisions

### D1 — where the read logic lives
`team.rs` / `coordination.rs` / `permissions.rs` live in **aida-cli** today; aida-server depends on **aida-core**. To avoid duplicating the parsing, **move the pure read models** (roster+roles, coordination-claim listing, effective-role) into **aida-core** (a `team`/`coordination` module) so both the CLI and the server consume one source of truth. If a clean move is too invasive for slice A, the fallback is the server reading the registry TOML/files directly with small parsers — but prefer the shared-core move (the dashboard is a real second consumer that justifies it, and it prevents CLI/server drift, the recurring STORY-82 hazard).

### D2 — endpoints (read-only, slice A)
- `GET /api/v2/team` → `{ members: TeamMemberDto[] }` where `TeamMemberDto = { user_id, role, hosts: string[], clone_paths: string[], last_seen, active_claim?: string }` (role from team.toml; identity/host from nodes.toml; active_claim joined from coordination/).
- `GET /api/v2/coordination` → `{ claims: CoordinationClaimDto[] }` where `CoordinationClaimDto = { kind: "lease"|"drain"|"solo", scope?, holder_user, host, clone_path, agent?, started_at, heartbeat_at, age_secs, stale: bool }`.
- DTOs derive ts-rs `TS` → exported in `aida-generate-types` → `shared/types.ts`, so the web app gets types for free.
- Respect the existing auth middleware + `X-Project` header + CORS exactly like the current v2 handlers.

### D3 — web views (slice B)
A **Team** page (`/team`, new Sidebar entry) with four panels, reusing existing component idioms:
- **Roster** — members table: user, role badge, host(s), last-seen, "active now" dot if holding a claim.
- **Assignment board** — specs grouped by assignee (columns per member + an Unassigned column); reuse the board/dnd idiom for read (drag-to-reassign is a stretch goal, gated on a `PUT assignee` endpoint — slice C).
- **Who holds what** — active coordination claims (leases/drains/solo) with holder + scope + age + stale flag.
- **Team burndown** — reuse `StatusChart` + `/analytics` (optionally per-assignee status breakdown).
- New React Query hooks `useTeam()` + `useCoordination()` over the new endpoints.

### D4 — no regressions / safety
- Read-only endpoints; no writes in slice A/B. Absent team.toml/coordination = empty arrays (the page shows "no team data yet"), never errors.
- Backward-compatible: the new endpoints + page are additive.

## 3. Slices

- **Slice A — server endpoints + DTOs (this first).** Move/share the read logic in aida-core (D1); add `GET /api/v2/team` + `/api/v2/coordination` (D2); ts-rs DTOs; endpoint unit/integration tests; regenerate `shared/types.ts`.
- **Slice B — web Team page (next).** `/team` route + Sidebar entry; `useTeam`/`useCoordination` hooks; the four panels (D3); typecheck + build green.
- **Slice C (stretch, later).** Interactivity: drag-to-reassign (needs `PUT /api/v2/requirements/:id/assignee`), set-role from the UI (guardrail caveat surfaced), live refresh.

## 4. Verification
- Server: `cargo test -p aida-server` (new endpoint tests); `cargo run --bin aida-generate-types` regenerates types clean.
- Web: `npm --prefix aida-web-react run build` (tsc + vite) green; `npm run lint` if present.
- Manual: `aida dev serve` → open the dashboard → Team page renders roster/claims/assignments from a real store.

## Critical files
- `aida-core/src/{team,coordination}.rs` (new shared read modules — moved from aida-cli) or aida-cli equivalents reused
- `aida-server/src/rest.rs` (new `/api/v2/team` + `/api/v2/coordination` handlers + router lines)
- `aida-generate-types/src/main.rs` (export the new DTOs)
- `aida-web-react/src/components/team/*` (new page + panels), `src/api/team.ts`, `src/hooks/useTeam.ts`, `src/components/layout/Sidebar.tsx`, `src/App.tsx`

<!-- trace:EPIC-47 | ai:claude -->
