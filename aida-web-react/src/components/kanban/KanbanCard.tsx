import { useSortable } from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import { ArrowUp, Minus, ArrowDown } from 'lucide-react';
import type { Requirement } from '@shared/types';
import { cn } from '../../lib/utils';
import { TypeBadge } from '../ui/Badge';
import { Avatar } from '../ui/Avatar';

const priorityIcons = {
  High: ArrowUp,
  Medium: Minus,
  Low: ArrowDown,
} as const;

const priorityColors = {
  High: 'text-red-400',
  Medium: 'text-amber-400',
  Low: 'text-gray-400',
} as const;

interface KanbanCardProps {
  requirement: Requirement;
  isDragOverlay?: boolean;
  highlighted?: boolean;
  selected?: boolean;
  onActivate?: (id: string) => void;
  onKeyAction?: (
    id: string,
    action: 'left' | 'right' | 'up' | 'down' | 'enter' | 'space' | 'escape',
  ) => void;
}

export function KanbanCard({
  requirement,
  isDragOverlay = false,
  highlighted = false,
  selected = false,
  onActivate,
  onKeyAction,
}: KanbanCardProps) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({
    id: requirement.id,
    data: {
      type: 'card',
      status: requirement.status,
    },
  });

  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
  };

  const PriorityIcon = priorityIcons[requirement.priority];

  return (
    <div
      ref={setNodeRef}
      id={`kanban-card-${requirement.id}`}
      data-kanban-card-id={requirement.id}
      style={style}
      {...attributes}
      {...listeners}
      onClick={() => {
        if (!isDragging && onActivate) onActivate(requirement.id);
      }}
      onKeyDown={(e) => {
        if (!onKeyAction) return;
        if (e.key === 'ArrowLeft') {
          e.preventDefault();
          onKeyAction(requirement.id, 'left');
        } else if (e.key === 'ArrowRight') {
          e.preventDefault();
          onKeyAction(requirement.id, 'right');
        } else if (e.key === 'ArrowUp') {
          e.preventDefault();
          onKeyAction(requirement.id, 'up');
        } else if (e.key === 'ArrowDown') {
          e.preventDefault();
          onKeyAction(requirement.id, 'down');
        } else if (e.key === 'Enter') {
          e.preventDefault();
          onKeyAction(requirement.id, 'enter');
        } else if (e.key === ' ') {
          e.preventDefault();
          onKeyAction(requirement.id, 'space');
        } else if (e.key === 'Escape') {
          e.preventDefault();
          onKeyAction(requirement.id, 'escape');
        }
      }}
      tabIndex={0}
      aria-keyshortcuts="ArrowLeft ArrowRight ArrowUp ArrowDown Enter Space Escape"
      aria-label={`Requirement ${requirement.spec_id ?? requirement.id}. Use arrow keys to move.`}
      className={cn(
        'rounded-lg border border-edge bg-surface-raised p-3 cursor-grab active:cursor-grabbing',
        'transition-shadow hover:border-edge-hover hover:shadow-md hover:shadow-black/10',
        'focus:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:border-accent focus-visible:bg-accent/5',
        isDragging && 'opacity-40',
        isDragOverlay && 'shadow-xl shadow-black/30 rotate-2 border-accent/50',
        highlighted && 'ring-2 ring-accent/70 border-accent animate-pulse',
        selected && 'ring-2 ring-blue-400/70 border-blue-400',
      )}
    >
      {/* Spec ID + Priority */}
      <div className="flex items-center justify-between mb-1.5">
        <span className="text-[11px] font-mono text-content-muted">{requirement.spec_id}</span>
        <PriorityIcon className={cn('h-3.5 w-3.5', priorityColors[requirement.priority])} />
      </div>

      {/* Title */}
      <h4 className="text-sm font-medium text-content leading-snug mb-2 line-clamp-2">
        {requirement.title}
      </h4>

      {/* Footer: type + owner */}
      <div className="flex items-center justify-between">
        <TypeBadge type={requirement.req_type} />
        {requirement.owner && (
          <Avatar name={requirement.owner} size="sm" />
        )}
      </div>
    </div>
  );
}
