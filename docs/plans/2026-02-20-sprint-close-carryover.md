# Sprint Management: Edit, Close, and Carry-Over

## Context

Sprints can be created and archived, but once created their metadata (dates, goal, velocity) can't be edited. There's no way to explicitly close a sprint or carry incomplete stories to the next sprint. No new backend work needed — the existing `PUT /api/v2/requirements/:id` already supports `custom_fields` updates.

## Implementation

### Phase 1: EditSprintModal
- New file: `EditSprintModal.tsx`
- Cloned CreateSprintModal pattern, pre-populates from `sprint.custom_fields`
- Uses `useUpdateRequirement()` to save

### Phase 2: CloseSprintModal
- New file: `CloseSprintModal.tsx`
- Summary section, incomplete items checklist, Close Sprint and Close & Create Next actions
- Sequential mutateAsync with per-step error handling

### Phase 3: SprintCard Action Buttons
- Pencil (edit), check-circle (close), archive icons in a row on hover

### Phase 4: Wiring
- SprintSelector passes onEdit/onClose through
- SprintView manages modal state and handlers

## Related Requirements
- Sprint management feature set

## Status
Completed
