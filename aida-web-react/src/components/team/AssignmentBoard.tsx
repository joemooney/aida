import { useMemo } from 'react';
import type { Requirement } from '@shared/types';
import { displayId } from '../../lib/utils';
import { StatusBadge } from '../ui/Badge';
import { EmptyState } from '../ui/EmptyState';
import { ClipboardList } from 'lucide-react';

// trace:STORY-649 | ai:claude

const UNASSIGNED = '__unassigned__';

interface AssignmentBoardProps {
  requirements: Requirement[];
}

export function AssignmentBoard({ requirements }: AssignmentBoardProps) {
  const columns = useMemo(() => {
    // Only stateful work items belong on the board (skip structural/stateless types).
    const items = requirements.filter(
      (r) => r.req_type !== 'Folder' && r.req_type !== 'Meta' && !r.archived,
    );

    const groups = new Map<string, Requirement[]>();
    for (const req of items) {
      const key = req.assignee && req.assignee.trim() ? req.assignee : UNASSIGNED;
      const bucket = groups.get(key) ?? [];
      bucket.push(req);
      groups.set(key, bucket);
    }

    // Assignees alphabetically, then the Unassigned column last.
    const assignees = Array.from(groups.keys())
      .filter((k) => k !== UNASSIGNED)
      .sort((a, b) => a.localeCompare(b));
    const ordered = [...assignees];
    if (groups.has(UNASSIGNED)) ordered.push(UNASSIGNED);

    return ordered.map((key) => ({
      key,
      label: key === UNASSIGNED ? 'Unassigned' : key,
      cards: groups.get(key) ?? [],
    }));
  }, [requirements]);

  return (
    <div className="rounded-xl border border-edge bg-surface-alt p-6">
      <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-4">
        Assignment Board
      </h3>
      {columns.length === 0 ? (
        <EmptyState
          icon={<ClipboardList className="h-8 w-8" />}
          title="No assignments yet"
          description="Assign specs with `aida assign <spec> --to <user>` to see them grouped here."
        />
      ) : (
        <div className="flex gap-4 overflow-x-auto pb-2">
          {columns.map((col) => (
            <div key={col.key} className="w-64 shrink-0">
              <div className="flex items-center justify-between mb-2 px-1">
                <span className="text-sm font-medium text-content truncate">{col.label}</span>
                <span className="text-xs text-content-muted tabular-nums">{col.cards.length}</span>
              </div>
              <div className="space-y-2">
                {col.cards.map((req) => (
                  <div
                    key={req.id}
                    className="rounded-lg border border-edge bg-surface-raised p-3 transition-shadow hover:border-edge-hover hover:shadow-md hover:shadow-black/10"
                  >
                    <div className="flex items-center justify-between mb-1.5">
                      <span className="text-[11px] font-mono text-content-muted">{displayId(req)}</span>
                      <StatusBadge status={req.status} />
                    </div>
                    <h4 className="text-sm font-medium text-content leading-snug line-clamp-2">
                      {req.title}
                    </h4>
                  </div>
                ))}
                {col.cards.length === 0 && (
                  <div className="rounded-lg border border-dashed border-edge p-3 text-xs text-content-muted text-center">
                    No specs
                  </div>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
