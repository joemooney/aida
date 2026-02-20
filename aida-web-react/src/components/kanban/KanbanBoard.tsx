import { useState, useMemo, useCallback } from 'react';
import {
  DndContext,
  DragOverlay,
  closestCenter,
  PointerSensor,
  useSensor,
  useSensors,
  type DragStartEvent,
  type DragEndEvent,
} from '@dnd-kit/core';
import type { Requirement, RequirementStatus } from '@shared/types';
import { STATUS_ORDER } from '../../lib/constants';
import { useRequirements, useUpdateRequirement } from '../../hooks/useRequirements';
import { useFilters } from '../../hooks/useFilters';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';
import { KanbanColumn } from './KanbanColumn';
import { KanbanCard } from './KanbanCard';
import { KanbanFilterBar } from './KanbanFilterBar';
import { Columns3 } from 'lucide-react';

export function KanbanBoard() {
  const { data: requirements, isLoading, error } = useRequirements();
  const updateReq = useUpdateRequirement();
  const { applyFilters } = useFilters();
  const [activeId, setActiveId] = useState<string | null>(null);

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
  );

  const filtered = useMemo(
    () => (requirements ? applyFilters(requirements) : []),
    [requirements, applyFilters],
  );

  const columnMap = useMemo(() => {
    const map: Record<RequirementStatus, Requirement[]> = {
      Draft: [], Approved: [], Planned: [], InProgress: [], Completed: [], Rejected: [],
    };
    for (const req of filtered) {
      map[req.status]?.push(req);
    }
    return map;
  }, [filtered]);

  const activeReq = useMemo(
    () => filtered.find((r) => r.id === activeId) ?? null,
    [filtered, activeId],
  );

  const handleDragStart = useCallback((event: DragStartEvent) => {
    setActiveId(event.active.id as string);
  }, []);

  const handleDragEnd = useCallback(
    (event: DragEndEvent) => {
      setActiveId(null);
      const { active, over } = event;
      if (!over) return;

      const draggedReq = filtered.find((r) => r.id === active.id);
      if (!draggedReq) return;

      const targetStatus = over.id as RequirementStatus;
      if (STATUS_ORDER.includes(targetStatus) && draggedReq.status !== targetStatus) {
        updateReq.mutate({
          id: draggedReq.spec_id ?? draggedReq.id,
          data: { status: targetStatus },
        });
      }
    },
    [filtered, updateReq],
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
        title="Failed to load requirements"
        description="Make sure the AIDA server is running on port 8080."
      />
    );
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold text-content">Kanban Board</h1>
      </div>

      <KanbanFilterBar requirements={requirements ?? []} />

      {filtered.length === 0 ? (
        <EmptyState
          icon={<Columns3 className="h-10 w-10" />}
          title="No requirements found"
          description="Try adjusting your filters or add new requirements."
        />
      ) : (
        <DndContext
          sensors={sensors}
          collisionDetection={closestCenter}
          onDragStart={handleDragStart}
          onDragEnd={handleDragEnd}
        >
          <div className="flex gap-4 overflow-x-auto pb-4">
            {STATUS_ORDER.map((status) => (
              <KanbanColumn
                key={status}
                status={status}
                requirements={columnMap[status]}
              />
            ))}
          </div>

          <DragOverlay>
            {activeReq ? <KanbanCard requirement={activeReq} isDragOverlay /> : null}
          </DragOverlay>
        </DndContext>
      )}
    </div>
  );
}
