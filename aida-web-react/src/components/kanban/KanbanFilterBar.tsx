import { X } from 'lucide-react';
import type { Requirement, RequirementStatus, RequirementPriority, RequirementType } from '@shared/types';
import { STATUS_ORDER } from '../../lib/constants';
import { useFilters } from '../../hooks/useFilters';

interface KanbanFilterBarProps {
  requirements: Requirement[];
}

export function KanbanFilterBar({ requirements }: KanbanFilterBarProps) {
  const { filters, setFilter, clearFilters, activeFilterCount } = useFilters();

  const features = [...new Set(requirements.map((r) => r.feature).filter(Boolean))].sort();
  const owners = [...new Set(requirements.map((r) => r.owner).filter(Boolean))].sort();
  const types: RequirementType[] = [...new Set(requirements.map((r) => r.req_type))].sort();

  const selectClass =
    'rounded-lg border border-edge bg-surface px-2.5 py-1.5 text-xs text-content focus:border-accent focus:outline-none cursor-pointer';

  return (
    <div className="flex items-center gap-3 flex-wrap">
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

      {activeFilterCount > 0 && (
        <button
          onClick={clearFilters}
          className="flex items-center gap-1 rounded-lg px-2.5 py-1.5 text-xs text-content-muted hover:text-content hover:bg-surface-hover transition-colors cursor-pointer"
        >
          <X className="h-3 w-3" />
          Clear ({activeFilterCount})
        </button>
      )}
    </div>
  );
}
