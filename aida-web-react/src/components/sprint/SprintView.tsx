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
import { Zap, Plus, Eye, EyeOff } from 'lucide-react';
import type { Requirement } from '@shared/types';
import { useRequirements, useUpdateRequirement } from '../../hooks/useRequirements';
import { useAssignToSprint, useRemoveFromSprint } from '../../hooks/useSprints';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';
import { SprintSelector } from './SprintSelector';
import { SprintBoard } from './SprintBoard';
import { SprintItemCard } from './SprintItemCard';
import { CreateSprintModal } from './CreateSprintModal';
import { SprintCharts } from './charts/SprintCharts';
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
  const updateMutation = useUpdateRequirement();
  const [selectedSprintId, setSelectedSprintId] = useState<string | null>(null);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [showCreateModal, setShowCreateModal] = useState(false);
  const [showArchived, setShowArchived] = useState(false);

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
  );

  // All sprints (including archived) for charts
  const allSprints = useMemo(() => {
    if (!requirements) return [];
    return requirements
      .filter((r) => r.req_type === 'Sprint')
      .sort((a, b) => {
        const aNum = getSprintNumber(a) ?? Infinity;
        const bNum = getSprintNumber(b) ?? Infinity;
        return aNum - bNum;
      });
  }, [requirements]);

  // Visible sprints (respecting archive toggle)
  const sprints = useMemo(() => {
    if (showArchived) return allSprints;
    return allSprints.filter((r) => !r.archived);
  }, [allSprints, showArchived]);

  // Next sprint number for create modal
  const nextSprintNumber = useMemo(() => {
    const nums = allSprints.map(getSprintNumber).filter((n): n is number => n != null);
    return nums.length > 0 ? Math.max(...nums) + 1 : 1;
  }, [allSprints]);

  // Map: sprint UUID -> items assigned to it (includes all sprints for velocity chart)
  const allSprintItemsMap = useMemo(() => {
    if (!requirements) return {} as Record<string, Requirement[]>;
    const map: Record<string, Requirement[]> = {};
    for (const sprint of allSprints) {
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
  }, [requirements, allSprints]);

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
    return [...backlog, ...(selectedSprintId ? allSprintItemsMap[selectedSprintId] ?? [] : [])];
  }, [requirements, backlog, allSprintItemsMap, selectedSprintId]);

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

  const handleArchive = useCallback(
    (sprintId: string) => {
      updateMutation.mutate({ id: sprintId, data: { archived: true } });
    },
    [updateMutation],
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

  const archivedCount = allSprints.filter((s) => s.archived).length;

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-2">
        <Zap className="h-5 w-5 text-accent" />
        <h1 className="text-xl font-semibold text-content">Sprint Planning</h1>
        <div className="flex-1" />
        {archivedCount > 0 && (
          <button
            onClick={() => setShowArchived(!showArchived)}
            className="flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium text-content-secondary hover:bg-surface-hover transition-colors"
          >
            {showArchived ? <EyeOff className="h-3.5 w-3.5" /> : <Eye className="h-3.5 w-3.5" />}
            {showArchived ? 'Hide archived' : `Show archived (${archivedCount})`}
          </button>
        )}
        <button
          onClick={() => setShowCreateModal(true)}
          className="flex items-center gap-1.5 rounded-lg bg-accent px-3 py-1.5 text-xs font-medium text-white hover:bg-accent/90 transition-colors"
        >
          <Plus className="h-3.5 w-3.5" />
          New Sprint
        </button>
      </div>

      {sprints.length === 0 && !showCreateModal ? (
        <EmptyState
          icon={<Zap className="h-10 w-10" />}
          title="No sprints found"
          description="Create a Sprint to start planning."
        />
      ) : (
        <>
          <SprintSelector
            sprints={sprints}
            sprintItemsMap={allSprintItemsMap}
            selectedId={selectedSprintId}
            onSelect={setSelectedSprintId}
            onArchive={handleArchive}
          />

          {selectedSprintId && (
            <>
              <DndContext
                sensors={sensors}
                collisionDetection={pointerWithin}
                onDragStart={handleDragStart}
                onDragEnd={handleDragEnd}
              >
                <SprintBoard
                  backlog={backlog}
                  sprintItems={allSprintItemsMap[selectedSprintId] ?? []}
                  sprintId={selectedSprintId}
                  sprintTitle={sprintTitle}
                />

                <DragOverlay>
                  {activeReq ? <SprintItemCard requirement={activeReq} isDragOverlay /> : null}
                </DragOverlay>
              </DndContext>

              {selectedSprint && (
                <SprintCharts
                  selectedSprint={selectedSprint}
                  sprintItems={allSprintItemsMap[selectedSprintId] ?? []}
                  allSprints={allSprints}
                  sprintItemsMap={allSprintItemsMap}
                />
              )}
            </>
          )}
        </>
      )}

      {showCreateModal && (
        <CreateSprintModal
          nextSprintNumber={nextSprintNumber}
          onClose={() => setShowCreateModal(false)}
        />
      )}
    </div>
  );
}
