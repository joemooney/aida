import { Sparkles, Pencil, MessageSquare, ExternalLink } from 'lucide-react';
import { Avatar } from '../ui/Avatar';
import { formatDate, formatRelativeDate } from '../../lib/utils';
import { useDetailPanel } from '../../hooks/useDetailPanel';
import type { TimelineEvent } from '../../lib/timeline-utils';

interface TimelineDetailPanelProps {
  event: TimelineEvent;
}

export function TimelineDetailPanel({ event }: TimelineDetailPanelProps) {
  const { open } = useDetailPanel();

  return (
    <div className="rounded-lg border border-edge bg-surface p-4">
      {/* Header */}
      <div className="flex items-start gap-3 mb-4">
        {event.eventType === 'Created' && <Sparkles className="h-5 w-5 text-green-500 mt-0.5 shrink-0" />}
        {event.eventType === 'Modified' && <Pencil className="h-5 w-5 text-blue-500 mt-0.5 shrink-0" />}
        {event.eventType === 'CommentAdded' && <MessageSquare className="h-5 w-5 text-amber-500 mt-0.5 shrink-0" />}
        <div className="min-w-0 flex-1">
          <div className="text-sm font-semibold text-content">{event.eventType}</div>
          <div className="text-xs text-content-muted mt-0.5">
            {formatRelativeDate(event.timestamp)} · {formatDate(event.timestamp)}
          </div>
        </div>
      </div>

      {/* Requirement link */}
      <div className="mb-4">
        <button
          type="button"
          onClick={() => open(event.specId)}
          className="inline-flex items-center gap-1.5 text-sm font-mono text-accent hover:underline cursor-pointer"
        >
          {event.specId}
          <ExternalLink className="h-3 w-3" />
        </button>
        <div className="text-sm text-content-secondary mt-0.5">{event.reqTitle}</div>
      </div>

      {/* Author */}
      <div className="flex items-center gap-2 mb-4">
        <Avatar name={event.author} size="sm" />
        <span className="text-sm text-content">{event.author}</span>
      </div>

      {/* Event-specific content */}
      {event.eventType === 'Modified' && event.changes.length > 0 && (
        <div>
          <h4 className="text-xs font-semibold text-content-muted uppercase tracking-wider mb-2">Changes</h4>
          <div className="space-y-2">
            {event.changes.map((change, i) => (
              <div key={i} className="rounded-md border border-edge bg-surface-alt p-2.5 text-xs">
                <div className="font-medium text-content mb-1">{change.field_name}</div>
                <div className="flex items-start gap-2">
                  <span className="text-red-400 line-through break-all">{change.old_value || '(empty)'}</span>
                  <span className="text-content-muted shrink-0">&rarr;</span>
                  <span className="text-green-400 break-all">{change.new_value || '(empty)'}</span>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {event.eventType === 'CommentAdded' && event.commentContent && (
        <div>
          <h4 className="text-xs font-semibold text-content-muted uppercase tracking-wider mb-2">Comment</h4>
          <div className="rounded-md border border-edge bg-surface-alt p-3 text-sm text-content whitespace-pre-wrap">
            {event.commentContent}
          </div>
        </div>
      )}
    </div>
  );
}
