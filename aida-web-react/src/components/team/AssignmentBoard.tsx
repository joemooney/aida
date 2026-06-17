import { useMemo, useState } from 'react';
import {
  DndContext,
  DragOverlay,
  PointerSensor,
  pointerWithin,
  useDraggable,
  useDroppable,
  useSensor,
  useSensors,
  type DragEndEvent,
  type DragStartEvent,
} from '@dnd-kit/core';
import type { Requirement, TeamMemberDto } from '@shared/types';
import { cn, displayId } from '../../lib/utils';
import { StatusBadge } from '../ui/Badge';
import { EmptyState } from '../ui/EmptyState';
import { ClipboardList } from 'lucide-react';
import { useReassign } from '../../hooks/useTeam';
import { usePermissions } from '../../hooks/usePermissions';

// trace:STORY-649 | ai:claude
// trace:STORY-651 | ai:claude
// Slice C2: the assignment board is now interactive — drag a spec card to
// another member's column (or Unassigned) to reassign it via PUT assignee, with
// an optimistic update + rollback handled by useReassign().

const UNASSIGNED = '__unassigned__';

interface AssignmentBoardProps {
  requirements: Requirement[];
  /** Roster members, so every member gets a drop column even with no specs. */
  members?: TeamMemberDto[];
}

interface BoardCardProps {
  req: Requirement;
  draggable: boolean;
}

function BoardCard({ req, draggable }: BoardCardProps) {
  const { attributes, listeners, setNodeRef, isDragging } = useDraggable({
    id: req.id,
    disabled: !draggable,
  });

  return (
    <div
      ref={setNodeRef}
      {...(draggable ? attributes : {})}
      {...(draggable ? listeners : {})}
      className={cn(
        'rounded-lg border border-edge bg-surface-raised p-3 transition-shadow',
        'hover:border-edge-hover hover:shadow-md hover:shadow-black/10',
        draggable && 'cursor-grab active:cursor-grabbing',
        isDragging && 'opacity-40',
      )}
    >
      <div className="flex items-center justify-between mb-1.5">
        <span className="text-[11px] font-mono text-content-muted">{displayId(req)}</span>
        <StatusBadge status={req.status} />
      </div>
      <h4 className="text-sm font-medium text-content leading-snug line-clamp-2">{req.title}</h4>
    </div>
  );
}

interface BoardColumnProps {
  columnKey: string;
  label: string;
  cards: Requirement[];
  draggable: boolean;
}

function BoardColumn({ columnKey, label, cards, draggable }: BoardColumnProps) {
  const { setNodeRef, isOver } = useDroppable({ id: columnKey });

  return (
    <div className="w-64 shrink-0">
      <div className="flex items-center justify-between mb-2 px-1">
        <span className="text-sm font-medium text-content truncate">{label}</span>
        <span className="text-xs text-content-muted tabular-nums">{cards.length}</span>
      </div>
      <div
        ref={setNodeRef}
        className={cn(
          'space-y-2 rounded-lg border border-transparent p-1 transition-colors min-h-[64px]',
          isOver && 'border-accent bg-accent/10',
        )}
      >
        {cards.map((req) => (
          <BoardCard key={req.id} req={req} draggable={draggable} />
        ))}
        {cards.length === 0 && (
          <div
            className={cn(
              'rounded-lg border border-dashed border-edge p-3 text-xs text-content-muted text-center',
              isOver && 'border-accent text-accent',
            )}
          >
            {isOver ? `Drop to assign to ${label}` : 'No specs'}
          </div>
        )}
      </div>
    </div>
  );
}

export function AssignmentBoard({ requirements, members = [] }: AssignmentBoardProps) {
  const { canWrite } = usePermissions();
  const reassign = useReassign();
  const [activeId, setActiveId] = useState<string | null>(null);

  const sensors = useSensors(
    // A small activation distance lets clicks through while still capturing drags.
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
  );

  const columns = useMemo(() => {
    // Only stateful work items belong on the board (skip structural/stateless types).
    const items = requirements.filter(
      (r) => r.req_type !== 'Folder' && r.req_type !== 'Meta' && !r.archived,
    );

    const groups = new Map<string, Requirement[]>();
    for (const req of items) {
      const key = req.assignee && req.assignee.trim() ? req.assignee : UNASSIGNED;
      const bucket = groups.get(key) ?? [];
      bucket.push(req);
      groups.set(key, bucket);
    }

    // Every roster member gets a column even with no specs, so they are valid
    // drop targets. trace:STORY-651 | ai:claude
    for (const m of members) {
      if (m.userId && !groups.has(m.userId)) groups.set(m.userId, []);
    }

    // Assignees alphabetically, then the Unassigned column last.
    const assignees = Array.from(groups.keys())
      .filter((k) => k !== UNASSIGNED)
      .sort((a, b) => a.localeCompare(b));
    const ordered = [...assignees, UNASSIGNED];

    return ordered.map((key) => ({
      key,
      label: key === UNASSIGNED ? 'Unassigned' : key,
      cards: groups.get(key) ?? [],
    }));
  }, [requirements, members]);

  const activeReq = useMemo(
    () => requirements.find((r) => r.id === activeId) ?? null,
    [requirements, activeId],
  );

  const handleDragStart = (event: DragStartEvent) => {
    setActiveId(event.active.id as string);
  };

  const handleDragEnd = (event: DragEndEvent) => {
    setActiveId(null);
    const { active, over } = event;
    if (!over) return;

    const reqId = active.id as string;
    const targetColumn = over.id as string;
    const req = requirements.find((r) => r.id === reqId);
    if (!req) return;

    const currentKey = req.assignee && req.assignee.trim() ? req.assignee : UNASSIGNED;
    if (currentKey === targetColumn) return; // dropped back on its own column

    const newAssignee = targetColumn === UNASSIGNED ? null : targetColumn;
    reassign.mutate({ id: req.spec_id ?? req.id, assignee: newAssignee });
  };

  // Cards with no real assignee value (empty string) can't drop-back-onto
  // themselves cleanly; dnd is gated on write permission either way.
  const draggable = canWrite;

  return (
    <div className="rounded-xl border border-edge bg-surface-alt p-6">
      <div className="flex items-center justify-between mb-4">
        <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted">
          Assignment Board
        </h3>
        {draggable && (
          <span className="text-[11px] text-content-muted">Drag a card to reassign</span>
        )}
      </div>
      {columns.length === 0 ? (
        <EmptyState
          icon={<ClipboardList className="h-8 w-8" />}
          title="No assignments yet"
          description="Assign specs with `aida assign <spec> --to <user>` to see them grouped here."
        />
      ) : (
        <DndContext
          sensors={sensors}
          collisionDetection={pointerWithin}
          onDragStart={handleDragStart}
          onDragEnd={handleDragEnd}
        >
          <div className="flex gap-4 overflow-x-auto pb-2">
            {columns.map((col) => (
              <BoardColumn
                key={col.key}
                columnKey={col.key}
                label={col.label}
                cards={col.cards}
                draggable={draggable}
              />
            ))}
          </div>
          <DragOverlay>
            {activeReq ? <BoardCard req={activeReq} draggable={false} /> : null}
          </DragOverlay>
        </DndContext>
      )}
      {reassign.isError && (
        <p className="mt-3 text-xs text-red-400">
          Reassignment failed — the board has been restored. Please try again.
        </p>
      )}
    </div>
  );
}
