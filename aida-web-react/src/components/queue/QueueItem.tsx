import { forwardRef, useCallback } from 'react';
import { useSortable } from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import { GripVertical, X } from 'lucide-react';
import type { QueueEntry } from '@shared/types';
import { StatusBadge, PriorityBadge } from '../ui/Badge';
import { cn } from '../../lib/utils';

// trace:STORY-0369 | ai:claude

interface QueueItemProps {
  entry: QueueEntry;
  index: number;
  userId: string;
  onRemove: (reqId: string) => void;
  onClick: (id: string) => void;
  isSelected?: boolean;
}

export const QueueItem = forwardRef<HTMLDivElement, QueueItemProps>(
function QueueItem({
  entry,
  index,
  userId,
  onRemove,
  onClick,
  isSelected,
}, ref) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: entry.requirementId });

  const setRefs = useCallback(
    (el: HTMLDivElement | null) => {
      setNodeRef(el);
      if (typeof ref === 'function') ref(el);
      else if (ref) ref.current = el;
    },
    [setNodeRef, ref],
  );

  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
  };

  return (
    <div
      ref={setRefs}
      style={style}
      className={cn(
        'group flex items-center gap-3 rounded-lg border border-edge bg-surface px-3 py-2.5 transition-colors hover:bg-surface-hover/50',
        isDragging && 'opacity-50 shadow-lg',
        isSelected && 'ring-2 ring-accent/40 bg-accent/5',
      )}
    >
      {/* Drag handle */}
      <button
        {...attributes}
        {...listeners}
        className="flex items-center text-content-muted hover:text-content cursor-grab active:cursor-grabbing shrink-0"
      >
        <GripVertical className="h-4 w-4" />
      </button>

      {/* Position number */}
      <span className="text-xs font-mono text-content-muted w-5 text-right shrink-0">
        {index + 1}
      </span>

      {/* Content — clickable */}
      <div
        className="flex-1 min-w-0 cursor-pointer"
        onClick={() => onClick(entry.specId ?? entry.requirementId)}
      >
        <div className="flex items-center gap-2">
          <span className="text-[11px] font-mono text-content-muted shrink-0">
            {entry.specId}
          </span>
          <span className="text-sm font-medium text-content truncate">
            {entry.title}
          </span>
        </div>
        {/* Added-by badge + note */}
        {(entry.addedBy !== userId || entry.note) && (
          <div className="flex items-center gap-2 mt-0.5">
            {entry.addedBy !== userId && (
              <span className="text-[10px] text-accent bg-accent/10 rounded px-1.5 py-0.5">
                from @{entry.addedBy}
              </span>
            )}
            {entry.note && (
              <span className="text-[10px] text-content-muted italic truncate">
                &ldquo;{entry.note}&rdquo;
              </span>
            )}
          </div>
        )}
      </div>

      {/* Badges */}
      <div className="flex items-center gap-2 shrink-0">
        <StatusBadge status={entry.status as any} />
        <PriorityBadge priority={entry.priority as any} />
      </div>

      {/* Remove button */}
      <button
        onClick={(e) => {
          e.stopPropagation();
          onRemove(entry.requirementId);
        }}
        className="flex h-6 w-6 items-center justify-center rounded text-content-muted opacity-0 group-hover:opacity-100 hover:text-red-500 hover:bg-red-500/10 transition-all cursor-pointer shrink-0"
        title="Remove from queue"
      >
        <X className="h-3.5 w-3.5" />
      </button>
    </div>
  );
});
