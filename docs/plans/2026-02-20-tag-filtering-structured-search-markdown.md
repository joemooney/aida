# Tag Filtering, Structured Search, and Markdown Descriptions

## Context

Three enhancements to the web dashboard:
1. **Tag filtering** — tags exist on requirements but can't be filtered on in the list/kanban/timeline views
2. **Structured search** — typing `owner:joe` or `tag:frontend` in the search bar should set filters rather than doing free-text search
3. **Markdown descriptions** — requirement descriptions should render as markdown when viewing (already have `react-markdown` + `remark-gfm` installed and used in docs/skills views)

All filtering is client-side (full requirement list already loaded). Filters are AND-only (all must match). No backend changes needed.

---

## Files to Modify

| File | Change |
|------|--------|
| `aida-web-react/src/hooks/useFilters.ts` | Add `tag` field, `removeFilter()`, tag matching |
| `aida-web-react/src/components/kanban/KanbanFilterBar.tsx` | Add tag dropdown, `FilterChip` component, active filter chips |
| `aida-web-react/src/components/layout/Header.tsx` | Parse `field:value` on Enter, apply as URL filters |
| `aida-web-react/src/components/list/RequirementsRow.tsx` | Show inline tag pills next to title |
| `aida-web-react/src/components/detail/DetailBody.tsx` | Render description as markdown when not editing |

No new files.

---

## Phase 1: Tag Filter in useFilters Hook

**File**: `aida-web-react/src/hooks/useFilters.ts`

1. Add `tag: string` to `Filters` interface
2. Read `tag` from `searchParams.get('tag')` in filters memo
3. Add to `applyFilters`: `if (filters.tag && !(req.tags ?? []).includes(filters.tag)) return false;`
4. Add `'tag'` to `clearFilters` deletions
5. Add `removeFilter(key: keyof Filters)` — deletes single key from URL params
6. Export `removeFilter`

## Phase 2: Filter Bar UI Enhancements

**File**: `aida-web-react/src/components/kanban/KanbanFilterBar.tsx`

1. Extract unique tags: `[...new Set(requirements.flatMap(r => r.tags ?? []).filter(Boolean))].sort()`
2. Add tag `<select>` dropdown after owner dropdown (same styling pattern)
3. Destructure `removeFilter` from `useFilters()`
4. Add local `FilterChip` component — accent-colored pill with `label:value` and X button
5. Replace "Clear (N)" button with active filter chip row:
   - One chip per active filter (status, priority, type, feature, owner, tag)
   - "Clear all" text button when 2+ filters active

## Phase 3: Structured Search in Header

**File**: `aida-web-react/src/components/layout/Header.tsx`

1. Import `useFilters` hook and `Filters` type
2. Add `parseStructuredQuery(input)` — regex parses `field:value` and `field:"quoted value"` patterns; returns `{ filters, remainder }`
3. Supported fields: `status`, `priority`, `type`, `feature`, `owner`, `tag`
4. Add `normalizeFilterValue(key, value)` — title-case status/priority, map type names (e.g., `bug` -> `Bug`)
5. On **Enter** keydown: parse query, apply detected filters via `setFilter()`, keep remainder as search text
6. Update placeholder: `'Search... (try owner:joe, tag:frontend)'`

Regular free-text search continues working on every keystroke as before. Structured parsing only triggers on Enter.

## Phase 4: Tag Pills in List Rows

**File**: `aida-web-react/src/components/list/RequirementsRow.tsx`

1. Wrap title cell content in a flex div
2. After title span, render up to 3 small tag badges from `requirement.tags`
3. Show `+N` overflow indicator if more than 3 tags

## Phase 5: Markdown Description Rendering

**File**: `aida-web-react/src/components/detail/DetailBody.tsx`

The `EditableText` component currently renders description as plain `whitespace-pre-wrap` text. Instead, replace the description section with:
- **View mode**: Render through `<Markdown remarkPlugins={[remarkGfm]}>` with the same prose styling used in `DocFullPage.tsx` (`prose prose-sm prose-invert max-w-none ...`). Click-to-edit pencil icon on hover.
- **Edit mode**: Keep the existing textarea behavior (raw markdown editing with Ctrl+Enter to save)

This means not using `EditableText` for description anymore — instead, inline the edit/view toggle directly in `DetailBody` for the description section only, so the view mode renders markdown.

Reuse the exact prose class string from `DocFullPage.tsx:56`:
```
prose prose-sm prose-invert max-w-none text-content prose-headings:text-content prose-strong:text-content prose-code:text-accent prose-code:bg-surface-hover prose-code:px-1.5 prose-code:py-0.5 prose-code:rounded prose-code:text-xs prose-code:before:content-none prose-code:after:content-none prose-pre:bg-surface-hover prose-pre:border prose-pre:border-edge prose-a:text-accent
```

---

## Verification

1. `npm run build` succeeds
2. Tag dropdown appears in filter bar, populated from requirement data
3. Selecting a tag filters list/kanban to matching requirements only
4. Typing `owner:joe` + Enter in search bar sets owner filter (dropdown updates, chip appears)
5. Typing `tag:frontend status:approved` + Enter sets both filters at once
6. Active filter chips appear below dropdowns; X on chip removes that filter
7. "Clear all" appears with 2+ filters
8. Tag pills show inline in list rows next to titles
9. Requirement description renders as markdown (headings, bold, links, code blocks)
10. Clicking description still opens textarea editor for raw markdown editing

## Related Requirements

- TBD

## Status

Completed
