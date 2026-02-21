import { useState, useMemo, useCallback } from 'react';
import { ArrowUpDown, List, GitBranch, ChevronsDownUp, ChevronsUpDown } from 'lucide-react';
import type { Requirement } from '@shared/types';
import { cn } from '../../lib/utils';
import { useRequirements } from '../../hooks/useRequirements';
import { useFilters } from '../../hooks/useFilters';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';
import { KanbanFilterBar } from '../kanban/KanbanFilterBar';
import { RequirementsRow } from './RequirementsRow';
import { TreeRow } from './TreeRow';
import { buildTree, flattenTree, collectParentIds } from '../../lib/tree-utils';

type SortKey = 'spec_id' | 'title' | 'status' | 'priority' | 'req_type' | 'owner' | 'modified_at';
type SortDir = 'asc' | 'desc';
type ViewMode = 'flat' | 'tree';

const columns: { key: SortKey; label: string; align?: 'right' }[] = [
  { key: 'spec_id', label: 'ID' },
  { key: 'title', label: 'Title' },
  { key: 'status', label: 'Status' },
  { key: 'priority', label: 'Priority' },
  { key: 'req_type', label: 'Type' },
  { key: 'owner', label: 'Owner' },
  { key: 'modified_at', label: 'Modified', align: 'right' },
];

function sortRequirements(reqs: Requirement[], key: SortKey, dir: SortDir): Requirement[] {
  return [...reqs].sort((a, b) => {
    const aVal = String(a[key] ?? '');
    const bVal = String(b[key] ?? '');
    const cmp = aVal.localeCompare(bVal, undefined, { numeric: true });
    return dir === 'asc' ? cmp : -cmp;
  });
}

export function RequirementsList() {
  const { data: requirements, isLoading, error } = useRequirements();
  const { applyFilters } = useFilters();
  const [sortKey, setSortKey] = useState<SortKey>('spec_id');
  const [sortDir, setSortDir] = useState<SortDir>('asc');
  const [viewMode, setViewMode] = useState<ViewMode>('flat');
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());

  const filtered = useMemo(
    () => (requirements ? applyFilters(requirements) : []),
    [requirements, applyFilters],
  );

  const sorted = useMemo(
    () => sortRequirements(filtered, sortKey, sortDir),
    [filtered, sortKey, sortDir],
  );

  // Build tree from ALL requirements, with filtered IDs for ancestor context
  const filteredIdSet = useMemo(
    () => new Set(filtered.map((r) => r.id)),
    [filtered],
  );

  const hasActiveFilters = requirements ? filtered.length !== requirements.length : false;

  const { roots, ancestorIds } = useMemo(
    () => buildTree(requirements ?? [], hasActiveFilters ? filteredIdSet : undefined),
    [requirements, hasActiveFilters, filteredIdSet],
  );

  const treeRows = useMemo(
    () => flattenTree(roots, collapsed, hasActiveFilters ? filteredIdSet : undefined, ancestorIds),
    [roots, collapsed, hasActiveFilters, filteredIdSet, ancestorIds],
  );

  const parentIds = useMemo(() => collectParentIds(roots), [roots]);

  const handleToggle = useCallback((id: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  }, []);

  const expandAll = useCallback(() => setCollapsed(new Set()), []);
  const collapseAll = useCallback(() => setCollapsed(new Set(parentIds)), [parentIds]);

  function handleSort(key: SortKey) {
    if (sortKey === key) {
      setSortDir((d) => (d === 'asc' ? 'desc' : 'asc'));
    } else {
      setSortKey(key);
      setSortDir('asc');
    }
  }

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64">
        <Spinner size="lg" />
      </div>
    );
  }

  if (error) {
    return (
      <EmptyState
        title="Failed to load requirements"
        description="Make sure the AIDA server is running on port 8080."
      />
    );
  }

  const isTree = viewMode === 'tree';
  const displayCount = isTree ? treeRows.length : sorted.length;

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <h1 className="text-xl font-semibold text-content">Requirements</h1>
          <div className="flex items-center rounded-lg border border-edge overflow-hidden">
            <button
              onClick={() => setViewMode('flat')}
              className={cn(
                'flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium transition-colors',
                !isTree
                  ? 'bg-accent text-white'
                  : 'text-content-muted hover:text-content hover:bg-surface-hover',
              )}
              title="Flat list"
            >
              <List className="h-3.5 w-3.5" />
              List
            </button>
            <button
              onClick={() => setViewMode('tree')}
              className={cn(
                'flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium transition-colors',
                isTree
                  ? 'bg-accent text-white'
                  : 'text-content-muted hover:text-content hover:bg-surface-hover',
              )}
              title="Parent/child tree"
            >
              <GitBranch className="h-3.5 w-3.5" />
              Tree
            </button>
          </div>
          {isTree && parentIds.size > 0 && (
            <div className="flex items-center gap-1">
              <button
                onClick={expandAll}
                className="flex items-center gap-1 px-2 py-1 text-xs text-content-muted hover:text-content transition-colors"
                title="Expand all"
              >
                <ChevronsUpDown className="h-3.5 w-3.5" />
              </button>
              <button
                onClick={collapseAll}
                className="flex items-center gap-1 px-2 py-1 text-xs text-content-muted hover:text-content transition-colors"
                title="Collapse all"
              >
                <ChevronsDownUp className="h-3.5 w-3.5" />
              </button>
            </div>
          )}
        </div>
        <span className="text-sm text-content-muted">{displayCount} items</span>
      </div>

      <KanbanFilterBar requirements={requirements ?? []} />

      {displayCount === 0 ? (
        <EmptyState
          icon={<List className="h-10 w-10" />}
          title="No requirements found"
          description="Try adjusting your filters."
        />
      ) : (
        <div className="rounded-xl border border-edge bg-surface-alt overflow-hidden">
          <div className="overflow-x-auto">
            <table className="w-full">
              <thead>
                <tr className="border-b border-edge">
                  {columns.map((col) => (
                    <th
                      key={col.key}
                      onClick={isTree ? undefined : () => handleSort(col.key)}
                      className={cn(
                        'px-4 py-3 text-xs font-medium uppercase tracking-wider text-content-muted whitespace-nowrap',
                        col.align === 'right' ? 'text-right' : 'text-left',
                        isTree ? 'cursor-default' : 'cursor-pointer hover:text-content transition-colors',
                      )}
                    >
                      <span className="inline-flex items-center gap-1">
                        {col.label}
                        {!isTree && sortKey === col.key && (
                          <ArrowUpDown className="h-3 w-3 text-accent" />
                        )}
                      </span>
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {isTree
                  ? treeRows.map((row) => (
                      <TreeRow
                        key={row.node.requirement.id}
                        node={row.node}
                        isCollapsed={collapsed.has(row.node.requirement.id)}
                        onToggle={handleToggle}
                        isDimmed={row.isAncestorOnly}
                      />
                    ))
                  : sorted.map((req) => (
                      <RequirementsRow key={req.id} requirement={req} />
                    ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
}
