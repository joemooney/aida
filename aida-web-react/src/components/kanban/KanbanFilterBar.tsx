import { useEffect } from 'react';
import { Check, X } from 'lucide-react';
import type { Requirement, RequirementStatus, RequirementPriority, RequirementType } from '@shared/types';
import { STATUS_ORDER } from '../../lib/constants';
import { useFilters, type Filters } from '../../hooks/useFilters';

function FilterChip({ label, value, onRemove }: { label: string; value: string; onRemove: () => void }) {
  return (
    <span className="inline-flex items-center gap-1 rounded-md bg-accent/15 text-accent pl-2 pr-1 py-0.5 text-xs font-medium">
      {label}:{value}
      <button
        onClick={onRemove}
        className="rounded-sm p-0.5 hover:bg-accent/20 transition-colors cursor-pointer"
      >
        <X className="h-2.5 w-2.5" />
      </button>
    </span>
  );
}

interface KanbanFilterBarProps {
  requirements: Requirement[];
  selectedStatuses?: RequirementStatus[];
  onToggleStatus?: (status: RequirementStatus) => void;
  onSelectAllStatuses?: () => void;
}

export function KanbanFilterBar({
  requirements,
  selectedStatuses,
  onToggleStatus,
  onSelectAllStatuses,
}: KanbanFilterBarProps) {
  const { filters, setFilter, removeFilter, clearFilters, activeFilterCount } = useFilters();

  const features = [...new Set(requirements.map((r) => r.feature).filter(Boolean))].sort();
  const owners = [...new Set(requirements.map((r) => r.owner).filter(Boolean))].sort();
  const types: RequirementType[] = [...new Set(requirements.map((r) => r.req_type))].sort();
  const tags = [...new Set(requirements.flatMap((r) => r.tags ?? []).filter(Boolean))].sort();

  const selectClass =
    'rounded-lg border border-edge bg-surface px-2.5 py-1.5 text-xs text-content focus:border-accent focus:outline-none cursor-pointer';

  const filterLabels: Record<keyof Filters, string> = {
    status: 'status',
    priority: 'priority',
    type: 'type',
    feature: 'feature',
    owner: 'owner',
    tag: 'tag',
  };

  const activeFilters = (Object.keys(filters) as (keyof Filters)[]).filter((k) => filters[k]);
  const usingMultiStatus =
    !!selectedStatuses && !!onToggleStatus && !!onSelectAllStatuses;
  const allStatusesSelected =
    usingMultiStatus && selectedStatuses.length === STATUS_ORDER.length;

  useEffect(() => {
    // Status filtering is handled by Kanban local multi-select state.
    if (usingMultiStatus && filters.status) {
      setFilter('status', '');
    }
  }, [filters.status, setFilter, usingMultiStatus]);

  return (
    <div className="space-y-2">
      {usingMultiStatus ? (
        <div className="flex items-center gap-2 flex-wrap">
          <span className="text-xs text-content-muted">Statuses:</span>
          {STATUS_ORDER.map((status) => {
            const selected = selectedStatuses.includes(status);
            return (
              <button
                key={status}
                onClick={() => onToggleStatus(status)}
                className={
                  selected
                    ? 'inline-flex items-center gap-1 rounded-md bg-accent/15 text-accent px-2 py-1 text-xs font-medium border border-accent/40 cursor-pointer'
                    : 'inline-flex items-center gap-1 rounded-md bg-surface border border-edge text-content-muted px-2 py-1 text-xs font-medium hover:text-content hover:border-edge-hover cursor-pointer'
                }
                title={selected ? `Hide ${status}` : `Show ${status}`}
              >
                {selected ? <Check className="h-3 w-3" /> : null}
                {status}
              </button>
            );
          })}
          {!allStatusesSelected && (
            <button
              onClick={onSelectAllStatuses}
              className="text-xs text-content-muted hover:text-content transition-colors cursor-pointer ml-1"
            >
              Select all
            </button>
          )}
        </div>
      ) : null}

      <div className="flex items-center gap-3 flex-wrap">
        {!usingMultiStatus && (
          <select
            value={filters.status}
            onChange={(e) => setFilter('status', e.target.value as RequirementStatus | '')}
            className={selectClass}
          >
            <option value="">All Statuses</option>
            {STATUS_ORDER.map((s) => (
              <option key={s} value={s}>{s}</option>
            ))}
          </select>
        )}

        <select
          value={filters.priority}
          onChange={(e) => setFilter('priority', e.target.value as RequirementPriority | '')}
          className={selectClass}
        >
          <option value="">All Priorities</option>
          <option value="High">High</option>
          <option value="Medium">Medium</option>
          <option value="Low">Low</option>
        </select>

        <select
          value={filters.type}
          onChange={(e) => setFilter('type', e.target.value as RequirementType | '')}
          className={selectClass}
        >
          <option value="">All Types</option>
          {types.map((t) => (
            <option key={t} value={t}>{t}</option>
          ))}
        </select>

        <select
          value={filters.feature}
          onChange={(e) => setFilter('feature', e.target.value)}
          className={selectClass}
        >
          <option value="">All Features</option>
          {features.map((f) => (
            <option key={f} value={f}>{f}</option>
          ))}
        </select>

        <select
          value={filters.owner}
          onChange={(e) => setFilter('owner', e.target.value)}
          className={selectClass}
        >
          <option value="">All Owners</option>
          {owners.map((o) => (
            <option key={o} value={o}>{o}</option>
          ))}
        </select>

        <select
          value={filters.tag}
          onChange={(e) => setFilter('tag', e.target.value)}
          className={selectClass}
        >
          <option value="">All Tags</option>
          {tags.map((t) => (
            <option key={t} value={t}>{t}</option>
          ))}
        </select>
      </div>

      {activeFilterCount > 0 && (
        <div className="flex items-center gap-1.5 flex-wrap">
          {activeFilters.map((key) => (
            <FilterChip
              key={key}
              label={filterLabels[key]}
              value={filters[key]}
              onRemove={() => removeFilter(key)}
            />
          ))}
          {activeFilterCount >= 2 && (
            <button
              onClick={clearFilters}
              className="text-xs text-content-muted hover:text-content transition-colors cursor-pointer ml-1"
            >
              Clear all
            </button>
          )}
        </div>
      )}
    </div>
  );
}
