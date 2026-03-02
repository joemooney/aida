import { useState, useMemo } from 'react';
import { useSearchParams } from 'react-router-dom';
import { Activity } from 'lucide-react';
import { useRequirements } from '../../hooks/useRequirements';
import { useQueue } from '../../hooks/useQueue';
import { useDetailPanel } from '../../hooks/useDetailPanel';
import { useAuth } from '../../hooks/useAuth';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';
import { TimelineDetailPanel } from '../timeline/TimelineDetailPanel';
import { ActivityStatsBar } from './ActivityStatsBar';
import { ActivityDateGroup } from './ActivityDateGroup';
import {
  buildUserActivity,
  computeActivityStats,
  groupActivityByDate,
  type TimeRange,
} from '../../lib/activity-utils';

const selectClass =
  'rounded-lg border border-edge bg-surface px-2.5 py-1.5 text-xs text-content focus:border-accent focus:outline-none cursor-pointer';

const TIME_RANGE_OPTIONS: { value: TimeRange; label: string }[] = [
  { value: 'today', label: 'Today' },
  { value: 'week', label: 'This Week' },
  { value: 'month', label: 'This Month' },
  { value: 'all', label: 'All Time' },
];

export function ActivityPage() {
  const { authEnabled, user } = useAuth();
  const [searchParams, setSearchParams] = useSearchParams();
  const requestedUser = searchParams.get('user') || 'default';
  const ownUserId = authEnabled && user?.handle ? user.handle : 'default';
  const userId = requestedUser === 'default' ? ownUserId : requestedUser;
  const isOwnActivity = userId === ownUserId;

  const [timeRange, setTimeRange] = useState<TimeRange>('week');
  const [selectedEventId, setSelectedEventId] = useState<string | null>(null);

  const { data: requirements, isLoading: reqLoading, error: reqError } = useRequirements();
  const { data: queueData, isLoading: queueLoading } = useQueue(userId);
  const { open } = useDetailPanel();

  const owners = useMemo(
    () => [...new Set((requirements ?? []).map((r) => r.owner).filter(Boolean))].sort(),
    [requirements],
  );

  const queueEntries = queueData?.entries ?? [];

  const activityItems = useMemo(
    () =>
      requirements
        ? buildUserActivity(requirements, queueEntries, userId, timeRange)
        : [],
    [requirements, queueEntries, userId, timeRange],
  );

  const stats = useMemo(
    () => computeActivityStats(activityItems, queueEntries),
    [activityItems, queueEntries],
  );

  const dateGroups = useMemo(
    () => groupActivityByDate(activityItems),
    [activityItems],
  );

  const selectedEvent = useMemo(
    () =>
      selectedEventId
        ? activityItems.find((e) => e.id === selectedEventId) ?? null
        : null,
    [activityItems, selectedEventId],
  );

  const isLoading = reqLoading || queueLoading;

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full">
        <Spinner size="lg" />
      </div>
    );
  }

  if (reqError) {
    return (
      <div className="flex items-center justify-center h-full">
        <EmptyState
          title="Failed to load activity"
          description={reqError instanceof Error ? reqError.message : 'Unknown error'}
        />
      </div>
    );
  }

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="border-b border-edge px-6 py-4 space-y-4">
        <div className="flex items-center justify-between">
          <h1 className="text-lg font-semibold text-content flex items-center gap-2">
            <Activity className="h-5 w-5 text-accent" />
            {isOwnActivity ? 'My Activity' : `${userId}'s Activity`}
          </h1>
          <div className="flex items-center gap-3">
            <select
              value={userId}
              onChange={(e) => {
                const val = e.target.value;
                setSearchParams((prev) => {
                  const next = new URLSearchParams(prev);
                  if (val === 'default') {
                    next.delete('user');
                  } else {
                    next.set('user', val);
                  }
                  return next;
                });
              }}
              className={selectClass}
            >
              <option value="default">My Activity (default)</option>
              {owners.map((o) => (
                <option key={o} value={o}>{o}</option>
              ))}
            </select>
            <select
              value={timeRange}
              onChange={(e) => setTimeRange(e.target.value as TimeRange)}
              className={selectClass}
            >
              {TIME_RANGE_OPTIONS.map((opt) => (
                <option key={opt.value} value={opt.value}>{opt.label}</option>
              ))}
            </select>
          </div>
        </div>
        <ActivityStatsBar stats={stats} />
      </div>

      {/* Content */}
      <div className="flex flex-1 min-h-0">
        {/* Activity feed */}
        <div className="flex-1 overflow-y-auto p-4 space-y-4">
          {dateGroups.length === 0 ? (
            <EmptyState
              icon={<Activity className="h-10 w-10" />}
              title="No activity found"
              description="No events match the current filters. Try expanding the time range."
            />
          ) : (
            dateGroups.map((group) => (
              <ActivityDateGroup
                key={group.dateKey}
                group={group}
                selectedEventId={selectedEventId}
                onSelectEvent={setSelectedEventId}
                onOpenDetail={open}
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
