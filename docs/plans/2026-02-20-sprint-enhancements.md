# Sprint Enhancements: Create, Archive, and Charts

**Date**: 2026-02-20
**Commit**: 8f28d2a

## Status

Completed

## Summary

Added three enhancements to the Sprint View: creating new sprints from the UI, archiving sprints, and sprint charts (burndown, burn-up, velocity). Uses pure SVG rendering matching the existing codebase pattern. Extended the V2 API with `custom_fields` and `archived` support on update, and added a new `POST /api/v2/requirements` create endpoint.

## Phases

### Phase 1: Extend V2 API (Rust)
- Added `custom_fields` and `archived` fields to `UpdateRequirementV2Request`
- Added `CreateRequirementV2Request` struct and `create_requirement_v2_legacy` handler
- Registered `POST /api/v2/requirements` route

### Phase 2: Sprint API & Hooks (TypeScript)
- Added `createSprint()` API function calling `POST /v2/requirements`
- Added `useCreateSprint()` mutation hook
- Existing `updateRequirement` already supports `archived` via `Partial<Requirement>`

### Phase 3: Create Sprint Modal
- New `CreateSprintModal.tsx` with form: sprint number, title, start/end dates, goal, planned velocity
- Auto-suggests next sprint number, auto-generates title
- Escape to close, backdrop click to close

### Phase 4: Archive Sprint
- Added archive button (icon, hover-visible) to `SprintCard`
- Passed `onArchive` through `SprintSelector`
- Added "Show archived" toggle in `SprintView` header

### Phase 5: Sprint Charts (Pure SVG)
- `BurndownChart.tsx` - ideal vs actual remaining items
- `BurnupChart.tsx` - scope vs cumulative completed
- `VelocityChart.tsx` - bar chart of completed points per sprint with average line
- `SprintCharts.tsx` - container rendering all three in responsive grid
- Chart data computation utilities added to `sprint-utils.ts`

### Phase 6: Integration
- Integrated modal, archive toggle, and charts into `SprintView.tsx`
- Both `cargo build` and `npm run build` pass

## Files Changed

| Action | File |
|--------|------|
| Modified | `aida-server/src/rest.rs` |
| Modified | `aida-web-react/src/api/sprints.ts` |
| Modified | `aida-web-react/src/hooks/useSprints.ts` |
| Modified | `aida-web-react/src/lib/sprint-utils.ts` |
| Modified | `aida-web-react/src/components/sprint/SprintCard.tsx` |
| Modified | `aida-web-react/src/components/sprint/SprintSelector.tsx` |
| Modified | `aida-web-react/src/components/sprint/SprintView.tsx` |
| Created | `aida-web-react/src/components/sprint/CreateSprintModal.tsx` |
| Created | `aida-web-react/src/components/sprint/charts/BurndownChart.tsx` |
| Created | `aida-web-react/src/components/sprint/charts/BurnupChart.tsx` |
| Created | `aida-web-react/src/components/sprint/charts/VelocityChart.tsx` |
| Created | `aida-web-react/src/components/sprint/charts/SprintCharts.tsx` |

## Related Requirements

- FR-0227 (REST API)

## Design Decisions

- **Pure SVG** for charts instead of a charting library — keeps bundle small and matches existing `StatusChart.tsx` pattern
- **History-based** burndown/burn-up — scans `item.history` for status changes to "Completed", falls back to `modified_at`
- **Velocity** uses `weight` field (story points) with fallback to 1 point per item
- **V2 create endpoint** returns native `models::Requirement` JSON directly (not protobuf wrapper)
