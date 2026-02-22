# List View Drag-and-Drop: Queue + Tree Reparenting

## Related Requirements
- STORY-0375

## Summary
Add drag-and-drop to the List View for two capabilities:
1. **Drag to My Queue** — drag any requirement row to add it to personal queue
2. **Drag to reparent** (tree view only) — drag a row onto another row to change its parent

## Implementation

### Backend
- `PUT /api/v2/requirements/:id/parent` with `{ parent_id: string | null }`
- Uses `store.set_relationship()` / `store.remove_relationship()` for Parent type

### Frontend
- `setParent()` API function + `useSetParent()` mutation hook
- `isDescendant()` tree utility for circular reference prevention
- `RequirementsRow` + `TreeRow` — `useDraggable` with GripVertical drag handle
- `TreeRow` — also `useDroppable` with isOver highlight
- `RequirementsList` — `DndContext` with queue/root drop zones and drag overlay

### Files Changed
- `aida-server/src/rest.rs`
- `aida-web-react/src/api/requirements.ts`
- `aida-web-react/src/hooks/useRequirements.ts`
- `aida-web-react/src/lib/tree-utils.ts`
- `aida-web-react/src/components/list/RequirementsList.tsx`
- `aida-web-react/src/components/list/RequirementsRow.tsx`
- `aida-web-react/src/components/list/TreeRow.tsx`

## Status
Completed
