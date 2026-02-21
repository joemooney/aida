import { X } from 'lucide-react';

interface TimelineFilterBarProps {
  authorFilter: string;
  fieldFilter: string;
  onAuthorFilterChange: (value: string) => void;
  onFieldFilterChange: (value: string) => void;
  onClear: () => void;
  eventCount: number;
  totalCount: number;
}

export function TimelineFilterBar({
  authorFilter,
  fieldFilter,
  onAuthorFilterChange,
  onFieldFilterChange,
  onClear,
  eventCount,
  totalCount,
}: TimelineFilterBarProps) {
  const hasFilters = authorFilter !== '' || fieldFilter !== '';
  const inputClass =
    'rounded-lg border border-edge bg-surface px-2.5 py-1.5 text-xs text-content placeholder:text-content-muted focus:border-accent focus:outline-none';

  return (
    <div className="flex items-center gap-3 flex-wrap">
      <input
        type="text"
        value={authorFilter}
        onChange={(e) => onAuthorFilterChange(e.target.value)}
        placeholder="Filter by author..."
        className={inputClass}
      />
      <input
        type="text"
        value={fieldFilter}
        onChange={(e) => onFieldFilterChange(e.target.value)}
        placeholder="Filter by field..."
        className={inputClass}
      />
      <span className="text-xs text-content-muted">
        {eventCount === totalCount ? `${eventCount} events` : `${eventCount} / ${totalCount} events`}
      </span>
      {hasFilters && (
        <button
          onClick={onClear}
          className="flex items-center gap-1 rounded-lg px-2.5 py-1.5 text-xs text-content-muted hover:text-content hover:bg-surface-hover transition-colors cursor-pointer"
        >
          <X className="h-3 w-3" />
          Clear
        </button>
      )}
    </div>
  );
}
