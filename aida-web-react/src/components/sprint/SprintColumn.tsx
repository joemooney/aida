import { useDroppable } from '@dnd-kit/core';
import type { Requirement } from '@shared/types';
import { cn } from '../../lib/utils';
import { SprintItemCard } from './SprintItemCard';

interface SprintColumnProps {
  id: string;
  title: string;
  items: Requirement[];
  accent?: string;
}

export function SprintColumn({ id, title, items, accent }: SprintColumnProps) {
  const { setNodeRef, isOver } = useDroppable({ id });

  const totalPoints = items.reduce((sum, r) => sum + (r.weight ?? 0), 0);

  return (
    <div
      ref={setNodeRef}
      className={cn(
        'flex flex-col rounded-xl bg-surface-alt border border-edge flex-1 min-w-[320px] transition-colors',
        isOver && 'border-accent/50 bg-accent/5',
      )}
    >
      {/* Column header */}
      <div className="flex items-center gap-2 px-4 py-3 border-b border-edge">
        {accent && <span className={cn('h-2 w-2 rounded-full', accent)} />}
        <span className="text-sm font-semibold text-content">{title}</span>
        <span className="ml-auto flex items-center gap-2">
          {totalPoints > 0 && (
            <span className="rounded-full bg-accent/10 px-2 py-0.5 text-[11px] font-medium text-accent">
              {totalPoints} pts
            </span>
          )}
          <span className="rounded-full bg-surface-hover px-2 py-0.5 text-xs font-medium text-content-muted">
            {items.length}
          </span>
        </span>
      </div>

      {/* Cards */}
      <div className="flex-1 overflow-y-auto p-2 space-y-2 min-h-[200px]">
        {items.map((req) => (
          <SprintItemCard key={req.id} requirement={req} />
        ))}
        {items.length === 0 && (
          <div className="flex items-center justify-center h-24 text-sm text-content-muted">
            Drop items here
          </div>
        )}
      </div>
    </div>
  );
}
