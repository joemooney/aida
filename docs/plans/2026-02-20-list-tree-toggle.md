# Parent/Child Tree Toggle on List View

## Context

The GUI has 5 hierarchy perspectives; the web List View is flat-only. Adding a Parent/Child tree toggle gives hierarchical navigation without a new view. Requirements already have `relationships` with `rel_type: "Parent"` linking children to parents via `target_id`.

## Approach

Add a List/Tree toggle button in the header. When "Tree" is active, requirements render as an indented, collapsible tree based on Parent relationships. The existing table, sorting, and filtering remain in flat mode. In tree mode, sorting is disabled (tree order takes over) but filters still apply — with ancestor nodes shown dimmed so filtered items aren't orphaned.

## Implementation

### 1. Tree utility functions (`aida-web-react/src/lib/tree-utils.ts`)
- `TreeNode` type with requirement, children, depth
- `buildTree()`: scans relationships for `rel_type === "Parent"`, builds parent→children map, returns sorted root nodes
- `flattenTree()`: returns flat list of visible rows respecting collapsed state
- `collectParentIds()`: helper for expand/collapse all

### 2. Tree row component (`aida-web-react/src/components/list/TreeRow.tsx`)
- Mirrors RequirementsRow layout with indent and collapse chevron
- `paddingLeft = depth * 20px` for indentation
- ChevronRight/ChevronDown toggle for parent nodes
- `opacity-50` for dimmed ancestor-only context nodes

### 3. RequirementsList.tsx modifications
- `viewMode: 'flat' | 'tree'` state with toggle button group
- `collapsed: Set<string>` state for tree collapse
- Expand all / Collapse all buttons in tree mode
- Column sorting disabled in tree mode
- Tree rendering via TreeRow components

## Related Requirements
- TASK-014

## Status
Completed
