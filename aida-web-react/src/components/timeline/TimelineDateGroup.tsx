import type { DateGroup } from '../../lib/timeline-utils';
import { TimelineEventCard } from './TimelineEventCard';

interface TimelineDateGroupProps {
  group: DateGroup;
  selectedEventId: string | null;
  onSelectEvent: (id: string) => void;
  onDoubleClickEvent: (specId: string) => void;
}

export function TimelineDateGroup({ group, selectedEventId, onSelectEvent, onDoubleClickEvent }: TimelineDateGroupProps) {
  return (
    <div>
      <div className="sticky top-0 z-10 bg-surface-alt/95 backdrop-blur-sm px-1 py-2">
        <h3 className="text-xs font-semibold text-content-muted uppercase tracking-wider">
          {group.label}
        </h3>
      </div>
      <div className="space-y-1.5">
        {group.events.map((event) => (
          <TimelineEventCard
            key={event.id}
            event={event}
            selected={event.id === selectedEventId}
            onSelect={onSelectEvent}
            onDoubleClick={onDoubleClickEvent}
          />
        ))}
      </div>
    </div>
  );
}
