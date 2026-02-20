import { useSortable } from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import { ArrowUp, Minus, ArrowDown } from 'lucide-react';
import type { Requirement } from '@shared/types';
import { cn } from '../../lib/utils';
import { TypeBadge } from '../ui/Badge';
import { Avatar } from '../ui/Avatar';
import { useDetailPanel } from '../../hooks/useDetailPanel';

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
}

export function KanbanCard({ requirement, isDragOverlay = false }: KanbanCardProps) {
  const { open } = useDetailPanel();
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: requirement.id });

  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
  };

  const PriorityIcon = priorityIcons[requirement.priority];

  return (
    <div
      ref={setNodeRef}
      style={style}
      {...attributes}
      {...listeners}
      onClick={() => !isDragging && open(requirement.spec_id ?? requirement.id)}
      className={cn(
        'rounded-lg border border-edge bg-surface-raised p-3 cursor-grab active:cursor-grabbing',
        'transition-all hover:border-edge-hover hover:shadow-md hover:shadow-black/10',
        isDragging && 'opacity-40',
        isDragOverlay && 'shadow-xl shadow-black/30 rotate-2 border-accent/50',
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
