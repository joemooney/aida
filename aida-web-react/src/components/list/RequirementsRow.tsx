// trace:FR-98 | ai:claude
import { forwardRef, useCallback } from 'react';
import { useDraggable, useDroppable } from '@dnd-kit/core';
import { GripVertical, ListPlus } from 'lucide-react';
import type { Requirement } from '@shared/types';
import { cn } from '../../lib/utils';
import { StatusBadge, PriorityBadge, TypeBadge } from '../ui/Badge';
import { Avatar } from '../ui/Avatar';
import { formatRelativeDate } from '../../lib/utils';
import { useDetailPanel } from '../../hooks/useDetailPanel';
import { useAddToQueue } from '../../hooks/useQueue';
import { usePermissions } from '../../hooks/usePermissions';

interface RequirementsRowProps {
  requirement: Requirement;
  isSelected?: boolean;
  onClick?: () => void;
}

export const RequirementsRow = forwardRef<HTMLTableRowElement, RequirementsRowProps>(
  function RequirementsRow({ requirement, isSelected, onClick }, ref) {
    const { open } = useDetailPanel();
    const addToQueue = useAddToQueue();
    const { canWrite } = usePermissions();
    const {
      attributes,
      listeners,
      setNodeRef: setDragRef,
      transform,
      isDragging,
    } = useDraggable({ id: requirement.id });

    // Droppable so another requirement can be dropped on top of this one to
    // make the dragged requirement a child of this one. trace:FR-98 | ai:claude
    const {
      setNodeRef: setDropRef,
      isOver,
    } = useDroppable({ id: requirement.id });

    const style = transform
      ? { transform: `translate(${transform.x}px, ${transform.y}px)` }
      : undefined;

    const setRefs = useCallback(
      (el: HTMLTableRowElement | null) => {
        setDragRef(el);
        setDropRef(el);
        if (typeof ref === 'function') ref(el);
        else if (ref) ref.current = el;
      },
      [setDragRef, setDropRef, ref],
    );

    return (
      <tr
        ref={setRefs}
        style={style}
        onClick={() => {
          if (!isDragging) {
            onClick?.();
            open(requirement.spec_id ?? requirement.id);
          }
        }}
        className={cn(
          'border-b border-edge hover:bg-surface-hover/50 transition-colors cursor-pointer group',
          isDragging && 'opacity-40',
          isOver && !isSelected && 'ring-2 ring-accent/60 bg-accent/5',
          isSelected && 'ring-2 ring-accent/40 bg-accent/5',
        )}
      >
        <td className="py-3 px-1 w-8">
          <div
            {...attributes}
            {...listeners}
            className="flex items-center justify-center h-6 w-6 rounded text-content-muted opacity-0 group-hover:opacity-100 hover:text-content cursor-grab active:cursor-grabbing transition-opacity"
            onClick={(e) => e.stopPropagation()}
          >
            <GripVertical className="h-3.5 w-3.5" />
          </div>
        </td>
        <td className="py-3 px-4">
          <span className="text-[11px] font-mono text-content-muted" title={requirement.agreed_id ? `Node ID: ${requirement.spec_id}` : undefined}>{requirement.agreed_id || requirement.spec_id}</span>
        </td>
        <td className="py-3 px-4">
          <div className="flex items-center gap-2">
            <span className="text-sm font-medium text-content group-hover:text-accent transition-colors">
              {requirement.title}
            </span>
            {requirement.tags && requirement.tags.length > 0 && (
              <div className="flex items-center gap-1 shrink-0">
                {requirement.tags.slice(0, 3).map((tag) => (
                  <span key={tag} className="rounded bg-surface-hover px-1.5 py-0.5 text-[10px] text-content-muted">
                    {tag}
                  </span>
                ))}
                {requirement.tags.length > 3 && (
                  <span className="text-[10px] text-content-muted">+{requirement.tags.length - 3}</span>
                )}
              </div>
            )}
          </div>
        </td>
        <td className="py-3 px-4">
          <StatusBadge status={requirement.status} />
        </td>
        <td className="py-3 px-4">
          <PriorityBadge priority={requirement.priority} />
        </td>
        <td className="py-3 px-4">
          <TypeBadge type={requirement.req_type} />
        </td>
        <td className="py-3 px-4">
          {requirement.owner ? (
            <div className="flex items-center gap-2">
              <Avatar name={requirement.owner} size="sm" />
              <span className="text-xs text-content-secondary">{requirement.owner}</span>
            </div>
          ) : (
            <span className="text-xs text-content-muted">&mdash;</span>
          )}
        </td>
        <td className="py-3 px-4 text-right">
          <div className="flex items-center justify-end gap-1">
            <span className="text-xs text-content-muted">{formatRelativeDate(requirement.modified_at)}</span>
            {canWrite && (
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  addToQueue.mutate({ requirement_id: requirement.id });
                }}
                title="Add to queue"
                className="flex h-6 w-6 items-center justify-center rounded text-content-muted opacity-0 group-hover:opacity-100 hover:text-accent hover:bg-accent/10 transition-all cursor-pointer"
              >
                <ListPlus className="h-3.5 w-3.5" />
              </button>
            )}
          </div>
        </td>
      </tr>
    );
  },
);
