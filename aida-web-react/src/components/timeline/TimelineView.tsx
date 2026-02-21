import { useState, useMemo } from 'react';
import { Clock } from 'lucide-react';
import { useRequirements } from '../../hooks/useRequirements';
import { useDetailPanel } from '../../hooks/useDetailPanel';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';
import { TimelineFilterBar } from './TimelineFilterBar';
import { TimelineDateGroup } from './TimelineDateGroup';
import { TimelineDetailPanel } from './TimelineDetailPanel';
import {
  buildTimelineEvents,
  filterTimelineEvents,
  groupEventsByDate,
} from '../../lib/timeline-utils';

export function TimelineView() {
  const { data: requirements, isLoading, error } = useRequirements();
  const { open } = useDetailPanel();

  const [filterAuthor, setFilterAuthor] = useState('');
  const [filterField, setFilterField] = useState('');
  const [selectedEventId, setSelectedEventId] = useState<string | null>(null);

  const allEvents = useMemo(
    () => (requirements ? buildTimelineEvents(requirements) : []),
    [requirements],
  );

  const filteredEvents = useMemo(
    () => filterTimelineEvents(allEvents, filterAuthor, filterField),
    [allEvents, filterAuthor, filterField],
  );

  const dateGroups = useMemo(
    () => groupEventsByDate(filteredEvents),
    [filteredEvents],
  );

  const selectedEvent = useMemo(
    () => (selectedEventId ? allEvents.find((e) => e.id === selectedEventId) ?? null : null),
    [allEvents, selectedEventId],
  );

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full">
        <Spinner size="lg" />
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex items-center justify-center h-full">
        <EmptyState
          title="Failed to load timeline"
          description={error instanceof Error ? error.message : 'Unknown error'}
        />
      </div>
    );
  }

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="flex items-center justify-between border-b border-edge px-6 py-4">
        <div className="flex items-center gap-3">
          <Clock className="h-5 w-5 text-accent" />
          <h1 className="text-lg font-semibold text-content">Timeline</h1>
        </div>
        <TimelineFilterBar
          authorFilter={filterAuthor}
          fieldFilter={filterField}
          onAuthorFilterChange={setFilterAuthor}
          onFieldFilterChange={setFilterField}
          onClear={() => { setFilterAuthor(''); setFilterField(''); }}
          eventCount={filteredEvents.length}
          totalCount={allEvents.length}
        />
      </div>

      {/* Content */}
      <div className="flex flex-1 min-h-0">
        {/* Event list */}
        <div className="flex-1 overflow-y-auto p-4 space-y-4">
          {dateGroups.length === 0 ? (
            <EmptyState
              icon={<Clock className="h-10 w-10" />}
              title="No events found"
              description={filterAuthor || filterField ? 'Try adjusting your filters.' : 'Events will appear as requirements are created and modified.'}
            />
          ) : (
            dateGroups.map((group) => (
              <TimelineDateGroup
                key={group.dateKey}
                group={group}
                selectedEventId={selectedEventId}
                onSelectEvent={setSelectedEventId}
                onDoubleClickEvent={open}
              />
            ))
          )}
        </div>

        {/* Detail panel */}
        <div className="w-96 shrink-0 border-l border-edge overflow-y-auto p-4">
          {selectedEvent ? (
            <TimelineDetailPanel event={selectedEvent} />
          ) : (
            <div className="flex items-center justify-center h-full text-sm text-content-muted">
              Select an event to view details
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
