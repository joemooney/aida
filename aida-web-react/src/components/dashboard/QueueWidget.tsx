import { Link } from 'react-router-dom';
import { Inbox, ArrowRight } from 'lucide-react';
import { useQueue } from '../../hooks/useQueue';
import { useDetailPanel } from '../../hooks/useDetailPanel';
import { StatusBadge } from '../ui/Badge';

// trace:STORY-0370 | ai:claude

export function QueueWidget() {
  const { data } = useQueue('default');
  const { open } = useDetailPanel();

  const entries = data?.entries ?? [];
  // Don't render if queue is empty
  if (entries.length === 0) return null;

  const top5 = entries.slice(0, 5);

  return (
    <div className="rounded-xl border border-edge bg-surface p-5">
      {/* Header */}
      <div className="flex items-center justify-between mb-4">
        <div className="flex items-center gap-2">
          <Inbox className="h-4 w-4 text-accent" />
          <h3 className="text-sm font-semibold text-content">My Queue</h3>
          <span className="text-[10px] font-medium text-content-muted bg-surface-hover rounded-full px-2 py-0.5">
            {entries.length}
          </span>
        </div>
        <Link
          to="/queue"
          className="flex items-center gap-1 text-xs text-accent hover:text-accent/80 transition-colors"
        >
          View all
          <ArrowRight className="h-3 w-3" />
        </Link>
      </div>

      {/* Items */}
      <div className="space-y-1.5">
        {top5.map((entry) => (
          <button
            key={entry.requirementId}
            onClick={() => open(entry.specId ?? entry.requirementId)}
            className="flex items-center gap-3 w-full rounded-lg px-2.5 py-2 text-left hover:bg-surface-hover/50 transition-colors cursor-pointer"
          >
            <span className="text-[11px] font-mono text-content-muted shrink-0 w-20 truncate">
              {entry.specId}
            </span>
            <span className="text-sm text-content truncate flex-1">
              {entry.title}
            </span>
            <StatusBadge status={entry.status as any} />
          </button>
        ))}
      </div>
    </div>
  );
}
