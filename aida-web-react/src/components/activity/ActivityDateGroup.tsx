import type { DateGroup } from '../../lib/timeline-utils';
import type { ActivityItem } from '../../lib/activity-utils';
import { ActivityItemCard } from './ActivityItemCard';

interface ActivityDateGroupProps {
  group: DateGroup;
  selectedEventId: string | null;
  onSelectEvent: (id: string) => void;
  onOpenDetail: (specId: string) => void;
}

export function ActivityDateGroup({ group, selectedEventId, onSelectEvent, onOpenDetail }: ActivityDateGroupProps) {
  return (
    <div>
      <div className="sticky top-0 z-10 bg-surface-alt/95 backdrop-blur-sm px-1 py-2">
        <h3 className="text-xs font-semibold text-content-muted uppercase tracking-wider">
          {group.label}
          <span className="ml-2 text-content-muted/60">({group.events.length})</span>
        </h3>
      </div>
      <div className="space-y-1.5">
        {group.events.map((event) => (
          <ActivityItemCard
            key={event.id}
            item={event as ActivityItem}
            selected={event.id === selectedEventId}
            onSelect={onSelectEvent}
            onOpenDetail={onOpenDetail}
          />
        ))}
      </div>
    </div>
  );
}
