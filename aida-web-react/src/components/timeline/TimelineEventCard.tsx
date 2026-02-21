import { Sparkles, Pencil, MessageSquare } from 'lucide-react';
import { cn, formatRelativeDate } from '../../lib/utils';
import { Avatar } from '../ui/Avatar';
import type { TimelineEvent, TimelineEventType } from '../../lib/timeline-utils';

const EVENT_ICONS: Record<TimelineEventType, typeof Sparkles> = {
  Created: Sparkles,
  Modified: Pencil,
  CommentAdded: MessageSquare,
};

const EVENT_COLORS: Record<TimelineEventType, string> = {
  Created: 'text-green-500',
  Modified: 'text-blue-500',
  CommentAdded: 'text-amber-500',
};

interface TimelineEventCardProps {
  event: TimelineEvent;
  selected: boolean;
  onSelect: (id: string) => void;
  onDoubleClick: (specId: string) => void;
}

export function TimelineEventCard({ event, selected, onSelect, onDoubleClick }: TimelineEventCardProps) {
  const Icon = EVENT_ICONS[event.eventType];

  return (
    <button
      type="button"
      onClick={() => onSelect(event.id)}
      onDoubleClick={() => onDoubleClick(event.specId)}
      className={cn(
        'flex items-start gap-3 w-full rounded-lg border px-3 py-2.5 text-left transition-colors cursor-pointer',
        selected
          ? 'bg-accent/10 border-accent'
          : 'border-edge bg-surface hover:bg-surface-hover',
      )}
    >
      <div className={cn('mt-0.5 shrink-0', EVENT_COLORS[event.eventType])}>
        <Icon className="h-4 w-4" />
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 text-xs text-content-muted">
          <span>{formatRelativeDate(event.timestamp)}</span>
          <span>·</span>
          <span className="font-medium text-content-secondary">{event.eventType}</span>
        </div>
        <div className="mt-0.5 text-sm text-content truncate">
          <span className="font-mono text-xs text-accent">{event.specId}</span>
          {' '}
          <span className="text-content-secondary">{event.reqTitle}</span>
        </div>
      </div>
      <Avatar name={event.author} size="sm" />
    </button>
  );
}
