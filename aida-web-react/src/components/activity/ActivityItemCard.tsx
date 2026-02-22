import { Sparkles, Pencil, MessageSquare } from 'lucide-react';
import { cn, formatRelativeDate } from '../../lib/utils';
import { Avatar } from '../ui/Avatar';
import type { ActivityItem } from '../../lib/activity-utils';
import type { TimelineEventType } from '../../lib/timeline-utils';

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

interface ActivityItemCardProps {
  item: ActivityItem;
  selected: boolean;
  onSelect: (id: string) => void;
  onOpenDetail: (specId: string) => void;
}

export function ActivityItemCard({ item, selected, onSelect, onOpenDetail }: ActivityItemCardProps) {
  const Icon = EVENT_ICONS[item.eventType];

  return (
    <button
      type="button"
      onClick={() => onSelect(item.id)}
      onDoubleClick={() => onOpenDetail(item.specId)}
      className={cn(
        'flex items-start gap-3 w-full rounded-lg border px-3 py-2.5 text-left transition-colors cursor-pointer',
        selected
          ? 'bg-accent/10 border-accent'
          : 'border-edge bg-surface hover:bg-surface-hover',
      )}
    >
      <div className={cn('mt-0.5 shrink-0', EVENT_COLORS[item.eventType])}>
        <Icon className="h-4 w-4" />
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 text-xs text-content-muted">
          <span>{formatRelativeDate(item.timestamp)}</span>
          <span>·</span>
          <span className="font-medium text-content-secondary">{item.eventType}</span>
          {item.inQueue && (
            <span className="rounded-full bg-green-500/15 text-green-600 px-1.5 py-0.5 text-[10px] font-medium">
              In Queue
            </span>
          )}
        </div>
        <div className="mt-0.5 text-sm text-content truncate">
          <span className="font-mono text-xs text-accent">{item.specId}</span>
          {' '}
          <span className="text-content-secondary">{item.reqTitle}</span>
        </div>
        {item.eventType === 'Modified' && item.changes.length > 0 && (
          <div className="mt-0.5 text-xs text-content-muted truncate">
            Changed {item.changes.map((c) => c.field_name).join(', ')}
          </div>
        )}
        {item.eventType === 'CommentAdded' && item.commentContent && (
          <div className="mt-0.5 text-xs text-content-muted truncate">
            {item.commentContent}
          </div>
        )}
      </div>
      <Avatar name={item.author} size="sm" />
    </button>
  );
}
