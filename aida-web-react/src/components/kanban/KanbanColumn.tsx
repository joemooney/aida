import { useDroppable } from '@dnd-kit/core';
import { SortableContext, verticalListSortingStrategy } from '@dnd-kit/sortable';
import { ChevronDown, ChevronRight } from 'lucide-react';
import type { RequirementStatus, Requirement } from '@shared/types';
import { cn } from '../../lib/utils';
import { STATUS_CONFIG } from '../../lib/constants';
import { KanbanCard } from './KanbanCard';

interface KanbanColumnProps {
  status: RequirementStatus;
  requirements: Requirement[];
  highlightedReqId?: string | null;
  selectedReqId?: string | null;
  collapsed?: boolean;
  onToggleCollapse?: (status: RequirementStatus) => void;
  onCardActivate?: (id: string) => void;
  onCardKeyAction?: (
    id: string,
    action: 'left' | 'right' | 'up' | 'down' | 'enter' | 'space' | 'escape',
  ) => void;
}

export function KanbanColumn({
  status,
  requirements,
  highlightedReqId = null,
  selectedReqId = null,
  collapsed = false,
  onToggleCollapse,
  onCardActivate,
  onCardKeyAction,
}: KanbanColumnProps) {
  const { setNodeRef, isOver } = useDroppable({ id: status });
  const config = STATUS_CONFIG[status];
  const itemIds = requirements.map((req) => req.id);

  return (
    <div
      ref={setNodeRef}
      className={cn(
        'flex flex-col rounded-xl bg-surface-alt border border-edge min-w-[280px] w-[280px] transition-colors',
        isOver && 'border-accent bg-accent/10',
      )}
    >
      {/* Column header */}
      <div className="flex items-center gap-2 px-4 py-3 border-b border-edge">
        <button
          onClick={() => onToggleCollapse?.(status)}
          className="rounded p-0.5 text-content-muted hover:text-content hover:bg-surface-hover transition-colors cursor-pointer"
          title={collapsed ? `Expand ${config.label}` : `Collapse ${config.label}`}
        >
          {collapsed ? <ChevronRight className="h-3.5 w-3.5" /> : <ChevronDown className="h-3.5 w-3.5" />}
        </button>
        <span className={cn('h-2 w-2 rounded-full', config.dot)} />
        <span className="text-sm font-semibold text-content">{config.label}</span>
        <span className="ml-auto rounded-full bg-surface-hover px-2 py-0.5 text-xs font-medium text-content-muted">
          {requirements.length}
        </span>
      </div>

      {/* Cards */}
      {!collapsed && (
        <SortableContext items={itemIds} strategy={verticalListSortingStrategy}>
          <div className="flex-1 overflow-y-auto p-2 space-y-2 min-h-[200px]">
            {isOver && requirements.length === 0 && (
              <div className="rounded-md border border-dashed border-accent/60 bg-accent/10 px-2 py-1 text-[11px] text-accent">
                Drop here to move to {config.label}
              </div>
            )}
            {requirements.map((req) => (
              <KanbanCard
                key={req.id}
                requirement={req}
                highlighted={highlightedReqId === req.id}
                selected={selectedReqId === req.id}
                onActivate={onCardActivate}
                onKeyAction={onCardKeyAction}
              />
            ))}
          </div>
        </SortableContext>
      )}
    </div>
  );
}
