import { useState, useMemo } from 'react';
import { ArrowUpDown } from 'lucide-react';
import type { Requirement } from '@shared/types';
import { cn } from '../../lib/utils';
import { useRequirements } from '../../hooks/useRequirements';
import { useFilters } from '../../hooks/useFilters';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';
import { KanbanFilterBar } from '../kanban/KanbanFilterBar';
import { RequirementsRow } from './RequirementsRow';
import { List } from 'lucide-react';

type SortKey = 'spec_id' | 'title' | 'status' | 'priority' | 'req_type' | 'owner' | 'modified_at';
type SortDir = 'asc' | 'desc';

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

  const filtered = useMemo(
    () => (requirements ? applyFilters(requirements) : []),
    [requirements, applyFilters],
  );

  const sorted = useMemo(
    () => sortRequirements(filtered, sortKey, sortDir),
    [filtered, sortKey, sortDir],
  );

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

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold text-content">Requirements</h1>
        <span className="text-sm text-content-muted">{sorted.length} items</span>
      </div>

      <KanbanFilterBar requirements={requirements ?? []} />

      {sorted.length === 0 ? (
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
                      onClick={() => handleSort(col.key)}
                      className={cn(
                        'px-4 py-3 text-xs font-medium uppercase tracking-wider text-content-muted cursor-pointer hover:text-content transition-colors whitespace-nowrap',
                        col.align === 'right' ? 'text-right' : 'text-left',
                      )}
                    >
                      <span className="inline-flex items-center gap-1">
                        {col.label}
                        {sortKey === col.key && (
                          <ArrowUpDown className="h-3 w-3 text-accent" />
                        )}
                      </span>
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {sorted.map((req) => (
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
