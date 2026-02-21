import { useState, useCallback } from 'react';
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
import { Inbox, Trash2 } from 'lucide-react';
import { useQueue, useRemoveFromQueue, useReorderQueue } from '../../hooks/useQueue';
import { useDetailPanel } from '../../hooks/useDetailPanel';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';
import { QueueItem } from './QueueItem';
import type { QueueEntry } from '@shared/types';

// trace:STORY-0369 | ai:claude

const USER_ID = 'default';

export function QueuePage() {
  const { data, isLoading, error } = useQueue(USER_ID);
  const removeFromQueue = useRemoveFromQueue(USER_ID);
  const reorderQueue = useReorderQueue(USER_ID);
  const { open } = useDetailPanel();
  const [activeId, setActiveId] = useState<string | null>(null);
  const [showCompleted, setShowCompleted] = useState(false);

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
  );

  const entries = data?.entries ?? [];
  const filtered = showCompleted
    ? entries
    : entries.filter((e) => e.status !== 'Completed');

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
            My Queue
            {filtered.length > 0 && (
              <span className="text-sm font-normal text-content-muted">
                ({filtered.length})
              </span>
            )}
          </h1>
          <p className="text-sm text-content-secondary mt-1">
            Your personal focus inbox — ordered by priority.
          </p>
        </div>
        <div className="flex items-center gap-3">
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
          title="Your queue is empty"
          description="Add items from the list view or detail panel to build your focus queue."
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
              {filtered.map((entry, index) => (
                <QueueItem
                  key={entry.requirementId}
                  entry={entry}
                  index={index}
                  userId={USER_ID}
                  onRemove={(reqId) => removeFromQueue.mutate(reqId)}
                  onClick={(id) => open(id)}
                />
              ))}
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
