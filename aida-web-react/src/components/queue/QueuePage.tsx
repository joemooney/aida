import { useState, useCallback, useMemo, useRef, useEffect } from 'react';
import { useSearchParams } from 'react-router-dom';
import {
  DndContext,
  DragOverlay,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type DragStartEvent,
  type DragEndEvent,
} from '@dnd-kit/core';
import {
  SortableContext,
  verticalListSortingStrategy,
  arrayMove,
} from '@dnd-kit/sortable';
import { Inbox } from 'lucide-react';
import { useQueue, useRemoveFromQueue, useReorderQueue } from '../../hooks/useQueue';
import { useRequirements } from '../../hooks/useRequirements';
import { useDetailPanel } from '../../hooks/useDetailPanel';
import { useListSelection } from '../../hooks/useListSelection';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';
import { QueueItem } from './QueueItem';
import type { QueueEntry } from '@shared/types';

// trace:STORY-0369 | ai:claude

const selectClass =
  'rounded-lg border border-edge bg-surface px-2.5 py-1.5 text-xs text-content focus:border-accent focus:outline-none cursor-pointer';

export function QueuePage() {
  const [searchParams, setSearchParams] = useSearchParams();
  const userId = searchParams.get('user') || 'default';
  const isOwnQueue = userId === 'default';

  const { data, isLoading, error } = useQueue(userId);
  const removeFromQueue = useRemoveFromQueue(userId);
  const reorderQueue = useReorderQueue(userId);
  const { data: requirements } = useRequirements();
  const { open } = useDetailPanel();
  const [activeId, setActiveId] = useState<string | null>(null);
  const [showCompleted, setShowCompleted] = useState(false);

  const owners = useMemo(
    () => [...new Set((requirements ?? []).map((r) => r.owner).filter(Boolean))].sort(),
    [requirements],
  );

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
  );

  const entries = data?.entries ?? [];
  const filtered = showCompleted
    ? entries
    : entries.filter((e) => e.status !== 'Completed');

  const displayItemIds = useMemo(
    () => filtered.map((e) => e.specId ?? e.requirementId),
    [filtered],
  );

  const { selectedId, setSelectedId } = useListSelection(displayItemIds);
  const selectedRowRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (selectedId && selectedRowRef.current) {
      selectedRowRef.current.scrollIntoView({ block: 'nearest' });
    }
  }, [selectedId]);

  const activeEntry = activeId
    ? entries.find((e) => e.requirementId === activeId) ?? null
    : null;

  const handleDragStart = useCallback((event: DragStartEvent) => {
    setActiveId(event.active.id as string);
  }, []);

  const handleDragEnd = useCallback(
    (event: DragEndEvent) => {
      setActiveId(null);
      const { active, over } = event;
      if (!over || active.id === over.id) return;

      const oldIndex = filtered.findIndex(
        (e) => e.requirementId === active.id,
      );
      const newIndex = filtered.findIndex(
        (e) => e.requirementId === over.id,
      );
      if (oldIndex === -1 || newIndex === -1) return;

      const reordered = arrayMove(filtered, oldIndex, newIndex);
      const items = reordered.map((entry, i) => ({
        requirement_id: entry.requirementId,
        position: (i + 1) * 1000,
      }));
      reorderQueue.mutate(items);
    },
    [filtered, reorderQueue],
  );

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64">
        <Spinner size="lg" />
      </div>
    );
  }

  if (error) {
    return (
      <EmptyState
        title="Failed to load queue"
        description="Make sure the AIDA server is running."
      />
    );
  }

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-xl font-semibold text-content flex items-center gap-2">
            <Inbox className="h-5 w-5" />
            {isOwnQueue ? 'My Queue' : `${userId}'s Queue`}
            {filtered.length > 0 && (
              <span className="text-sm font-normal text-content-muted">
                ({filtered.length})
              </span>
            )}
            {!isOwnQueue && (
              <span className="text-[10px] font-normal bg-amber-500/15 text-amber-600 rounded px-1.5 py-0.5">
                Read-only
              </span>
            )}
          </h1>
          <p className="text-sm text-content-secondary mt-1">
            {isOwnQueue
              ? 'Your personal focus inbox — ordered by priority.'
              : `Viewing ${userId}'s queue.`}
          </p>
        </div>
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
            <option value="default">My Queue (default)</option>
            {owners.map((o) => (
              <option key={o} value={o}>{o}</option>
            ))}
          </select>
          <label className="flex items-center gap-2 text-xs text-content-secondary cursor-pointer">
            <input
              type="checkbox"
              checked={showCompleted}
              onChange={(e) => setShowCompleted(e.target.checked)}
              className="rounded border-edge"
            />
            Show completed
          </label>
        </div>
      </div>

      {/* Queue list */}
      {filtered.length === 0 ? (
        <EmptyState
          icon={<Inbox className="h-10 w-10" />}
          title={isOwnQueue ? 'Your queue is empty' : `${userId}'s queue is empty`}
          description={isOwnQueue
            ? 'Add items from the list view or detail panel to build your focus queue.'
            : 'This user has no items in their queue.'}
        />
      ) : (
        <DndContext
          sensors={sensors}
          collisionDetection={closestCenter}
          onDragStart={handleDragStart}
          onDragEnd={handleDragEnd}
        >
          <SortableContext
            items={filtered.map((e) => e.requirementId)}
            strategy={verticalListSortingStrategy}
          >
            <div className="space-y-1.5">
              {filtered.map((entry, index) => {
                const rowId = entry.specId ?? entry.requirementId;
                const isRowSelected = selectedId === rowId;
                return (
                  <QueueItem
                    key={entry.requirementId}
                    ref={isRowSelected ? selectedRowRef : undefined}
                    entry={entry}
                    index={index}
                    userId={userId}
                    onRemove={(reqId) => removeFromQueue.mutate(reqId)}
                    onClick={(id) => {
                      setSelectedId(rowId);
                      open(id);
                    }}
                    isSelected={isRowSelected}
                    readOnly={!isOwnQueue}
                  />
                );
              })}
            </div>
          </SortableContext>
          <DragOverlay>
            {activeEntry ? (
              <div className="rounded-lg border border-accent bg-surface px-3 py-2.5 shadow-lg opacity-90">
                <span className="text-sm font-medium text-content">
                  {activeEntry.title}
                </span>
              </div>
            ) : null}
          </DragOverlay>
        </DndContext>
      )}
    </div>
  );
}
