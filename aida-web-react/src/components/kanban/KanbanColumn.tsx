import { useDroppable } from '@dnd-kit/core';
import { SortableContext, verticalListSortingStrategy } from '@dnd-kit/sortable';
import type { RequirementStatus, Requirement } from '@shared/types';
import { cn } from '../../lib/utils';
import { STATUS_CONFIG } from '../../lib/constants';
import { KanbanCard } from './KanbanCard';

interface KanbanColumnProps {
  status: RequirementStatus;
  requirements: Requirement[];
}

export function KanbanColumn({ status, requirements }: KanbanColumnProps) {
  const { setNodeRef, isOver } = useDroppable({ id: status });
  const config = STATUS_CONFIG[status];

  return (
    <div
      ref={setNodeRef}
      className={cn(
        'flex flex-col rounded-xl bg-surface-alt border border-edge min-w-[280px] w-[280px]',
        isOver && 'border-accent/50 bg-accent/5',
      )}
    >
      {/* Column header */}
      <div className="flex items-center gap-2 px-4 py-3 border-b border-edge">
        <span className={cn('h-2 w-2 rounded-full', config.dot)} />
        <span className="text-sm font-semibold text-content">{config.label}</span>
        <span className="ml-auto rounded-full bg-surface-hover px-2 py-0.5 text-xs font-medium text-content-muted">
          {requirements.length}
        </span>
      </div>

      {/* Cards */}
      <div className="flex-1 overflow-y-auto p-2 space-y-2 min-h-[200px]">
        <SortableContext items={requirements.map((r) => r.id)} strategy={verticalListSortingStrategy}>
          {requirements.map((req) => (
            <KanbanCard key={req.id} requirement={req} />
          ))}
        </SortableContext>
      </div>
    </div>
  );
}
