import { useState, useMemo, useCallback, useEffect } from 'react';
import {
  DndContext,
  DragOverlay,
  pointerWithin,
  PointerSensor,
  useSensor,
  useSensors,
  type DragStartEvent,
  type DragEndEvent,
} from '@dnd-kit/core';
import { Zap } from 'lucide-react';
import type { Requirement } from '@shared/types';
import { useRequirements } from '../../hooks/useRequirements';
import { useAssignToSprint, useRemoveFromSprint } from '../../hooks/useSprints';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';
import { SprintSelector } from './SprintSelector';
import { SprintBoard } from './SprintBoard';
import { SprintItemCard } from './SprintItemCard';
import {
  isSprintAssignment,
  getSprintAssignmentTarget,
  getSprintNumber,
  getSprintState,
} from '../../lib/sprint-utils';

export function SprintView() {
  const { data: requirements, isLoading, error } = useRequirements();
  const assignMutation = useAssignToSprint();
  const removeMutation = useRemoveFromSprint();
  const [selectedSprintId, setSelectedSprintId] = useState<string | null>(null);
  const [activeId, setActiveId] = useState<string | null>(null);

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
  );

  // Derive sprints sorted by sprint_number
  const sprints = useMemo(() => {
    if (!requirements) return [];
    return requirements
      .filter((r) => r.req_type === 'Sprint' && !r.archived)
      .sort((a, b) => {
        const aNum = getSprintNumber(a) ?? Infinity;
        const bNum = getSprintNumber(b) ?? Infinity;
        return aNum - bNum;
      });
  }, [requirements]);

  // Map: sprint UUID -> items assigned to it
  const sprintItemsMap = useMemo(() => {
    if (!requirements) return {} as Record<string, Requirement[]>;
    const map: Record<string, Requirement[]> = {};
    for (const sprint of sprints) {
      map[sprint.id] = [];
    }
    for (const req of requirements) {
      if (req.req_type === 'Sprint' || req.req_type === 'Folder' || req.req_type === 'Meta') continue;
      const target = getSprintAssignmentTarget(req);
      if (target && map[target]) {
        map[target].push(req);
      }
    }
    return map;
  }, [requirements, sprints]);

  // Backlog: requirements not assigned to any sprint, excluding Sprint/Folder/Meta
  const backlog = useMemo(() => {
    if (!requirements) return [];
    return requirements.filter((r) => {
      if (r.req_type === 'Sprint' || r.req_type === 'Folder' || r.req_type === 'Meta') return false;
      if (r.archived) return false;
      return !r.relationships?.some(isSprintAssignment);
    });
  }, [requirements]);

  // Auto-select active sprint, or first sprint
  useEffect(() => {
    if (selectedSprintId && sprints.some((s) => s.id === selectedSprintId)) return;
    const active = sprints.find((s) => getSprintState(s) === 'active');
    if (active) {
      setSelectedSprintId(active.id);
    } else if (sprints.length > 0) {
      setSelectedSprintId(sprints[0].id);
    }
  }, [sprints, selectedSprintId]);

  const selectedSprint = useMemo(
    () => sprints.find((s) => s.id === selectedSprintId) ?? null,
    [sprints, selectedSprintId],
  );

  const allItems = useMemo(() => {
    if (!requirements) return [];
    return [...backlog, ...(selectedSprintId ? sprintItemsMap[selectedSprintId] ?? [] : [])];
  }, [requirements, backlog, sprintItemsMap, selectedSprintId]);

  const activeReq = useMemo(
    () => allItems.find((r) => r.id === activeId) ?? null,
    [allItems, activeId],
  );

  const handleDragStart = useCallback((event: DragStartEvent) => {
    setActiveId(event.active.id as string);
  }, []);

  const handleDragEnd = useCallback(
    (event: DragEndEvent) => {
      setActiveId(null);
      const { active, over } = event;
      if (!over || !selectedSprintId) return;

      const draggedReq = allItems.find((r) => r.id === active.id);
      if (!draggedReq) return;

      const targetId = over.id as string;
      const sourceIsBacklog = !draggedReq.relationships?.some(isSprintAssignment);
      const targetIsBacklog = targetId === 'backlog';

      // Same column — no-op
      if ((sourceIsBacklog && targetIsBacklog) || (!sourceIsBacklog && targetId === selectedSprintId)) {
        return;
      }

      const reqId = draggedReq.spec_id ?? draggedReq.id;

      if (targetIsBacklog) {
        removeMutation.mutate(reqId);
      } else {
        assignMutation.mutate({ reqId, sprintId: targetId });
      }
    },
    [allItems, selectedSprintId, assignMutation, removeMutation],
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

  const sprintTitle = selectedSprint
    ? (getSprintNumber(selectedSprint) != null
        ? `Sprint ${getSprintNumber(selectedSprint)}`
        : selectedSprint.title)
    : 'Sprint';

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-2">
        <Zap className="h-5 w-5 text-accent" />
        <h1 className="text-xl font-semibold text-content">Sprint Planning</h1>
      </div>

      {sprints.length === 0 ? (
        <EmptyState
          icon={<Zap className="h-10 w-10" />}
          title="No sprints found"
          description="Create a Sprint requirement to start planning."
        />
      ) : (
        <>
          <SprintSelector
            sprints={sprints}
            sprintItemsMap={sprintItemsMap}
            selectedId={selectedSprintId}
            onSelect={setSelectedSprintId}
          />

          {selectedSprintId && (
            <DndContext
              sensors={sensors}
              collisionDetection={pointerWithin}
              onDragStart={handleDragStart}
              onDragEnd={handleDragEnd}
            >
              <SprintBoard
                backlog={backlog}
                sprintItems={sprintItemsMap[selectedSprintId] ?? []}
                sprintId={selectedSprintId}
                sprintTitle={sprintTitle}
              />

              <DragOverlay>
                {activeReq ? <SprintItemCard requirement={activeReq} isDragOverlay /> : null}
              </DragOverlay>
            </DndContext>
          )}
        </>
      )}
    </div>
  );
}
