# Docs Browser View

## Summary
Add a "Docs" page to the React dashboard that browses and renders markdown files from the `docs/` directory (including `docs/plans/`), with read-only markdown rendering via react-markdown.

## Implementation

### Phase 1: Backend — Docs API (Rust)
- Added `GET /api/v2/docs` — lists all `.md` files recursively from `docs/`
- Added `GET /api/v2/docs/*path` — returns full markdown content by relative path
- Path traversal protection via canonicalization
- Section classification: files in `docs/plans/` → "plans", others → "docs"
- Title extraction from first `# heading` line

### Phase 2: Frontend API + Hooks
- `src/api/docs.ts` — `fetchDocs()` and `fetchDoc(path)` API functions
- `src/hooks/useDocs.ts` — `useDocs()` and `useDoc(path)` query hooks

### Phase 3: Docs View Components
- `DocsView.tsx` — Main view with search, section filters (All/Docs/Plans), grouped card grid
- `DocCard.tsx` — Card showing title, section badge, file path
- `DocDetailPanel.tsx` — Right-slide panel with rendered markdown (max-w-3xl, read-only)

### Phase 4: Routing + Sidebar
- Added `/docs` route in App.tsx
- Added Docs nav item with FileText icon in Sidebar

## Files Changed
| Action | File |
|--------|------|
| Modify | `aida-server/src/rest.rs` — 2 docs API endpoints + route registration |
| Create | `aida-web-react/src/api/docs.ts` |
| Create | `aida-web-react/src/hooks/useDocs.ts` |
| Create | `aida-web-react/src/components/docs/DocsView.tsx` |
| Create | `aida-web-react/src/components/docs/DocCard.tsx` |
| Create | `aida-web-react/src/components/docs/DocDetailPanel.tsx` |
| Modify | `aida-web-react/src/components/layout/Sidebar.tsx` |
| Modify | `aida-web-react/src/App.tsx` |

## Related Requirements
- Docs browser view for AIDA React dashboard

## Status
Completed
