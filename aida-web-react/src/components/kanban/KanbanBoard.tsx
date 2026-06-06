import { useState, useMemo, useCallback, useEffect, useRef } from 'react';
import { useSearchParams } from 'react-router-dom';
import type { RuleGroupType } from 'react-querybuilder';
import {
  DndContext,
  DragOverlay,
  pointerWithin,
  PointerSensor,
  useSensor,
  useSensors,
  type DragStartEvent,
  type DragOverEvent,
  type DragEndEvent,
} from '@dnd-kit/core';
import { arrayMove } from '@dnd-kit/sortable';
import type { Requirement, RequirementStatus } from '@shared/types';
import { STATUS_ORDER } from '../../lib/constants';
import { useRequirements, useUpdateRequirement } from '../../hooks/useRequirements';
import { useFilters } from '../../hooks/useFilters';
import { useDetailPanel } from '../../hooks/useDetailPanel';
import { useHotkeys, type HotkeyBinding } from '../../hooks/useHotkeys';
import { useAdvancedQuery } from '../../hooks/useAdvancedQuery';
import { buildQueryFields } from '../../lib/query-fields';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';
import { KanbanColumn } from './KanbanColumn';
import { KanbanCard } from './KanbanCard';
import { KanbanFilterBar } from './KanbanFilterBar';
import { AdvancedQueryBuilder } from '../filters/AdvancedQueryBuilder';
import { SavedViewsPicker, type SavedViewSettingsPatch } from '../filters/SavedViewsPicker';
import { Columns3, SlidersHorizontal } from 'lucide-react';
import { cn } from '../../lib/utils';
import { useSavedViews } from '../../hooks/useSavedViews';

// NeedsAttention (STORY-332) must appear in every RequirementStatus record
// literal or `tsc -b` fails; CI now guards this (TASK-224).
// trace:TASK-224 | ai:claude
function emptyColumns(): Record<RequirementStatus, string[]> {
  return {
    Draft: [],
    Approved: [],
    Planned: [],
    InProgress: [],
    NeedsAttention: [],
    Done: [],
    Completed: [],
    Rejected: [],
  };
}

const EMPTY_ADVANCED_QUERY: RuleGroupType = { combinator: 'and', rules: [] };

export function KanbanBoard() {
  const { data: requirements, isLoading, error } = useRequirements();
  const updateReq = useUpdateRequirement();
  const [searchParams, setSearchParams] = useSearchParams();
  const { applyFilters } = useFilters();
  const { detailId, detailMode, open } = useDetailPanel();
  const {
    query: advancedQuery,
    onQueryChange,
    clearQuery,
    isOpen: advancedOpen,
    toggleOpen: toggleAdvanced,
    applyAdvancedFilter,
    hasActiveQuery,
    savedQueries,
    saveQuery,
    loadSavedQuery,
    deleteSavedQuery,
  } = useAdvancedQuery();
  const [activeId, setActiveId] = useState<string | null>(null);
  const [selectedCardId, setSelectedCardId] = useState<string | null>(null);
  const [mode, setMode] = useState<'navigation' | 'move'>('navigation');
  const [showFilterBar, setShowFilterBar] = useState(true);
  const [selectedStatuses, setSelectedStatuses] = useState<RequirementStatus[]>([...STATUS_ORDER]);
  const [collapsedStatuses, setCollapsedStatuses] = useState<Record<RequirementStatus, boolean>>({
    Draft: false,
    Approved: false,
    Planned: false,
    InProgress: false,
    NeedsAttention: false,
    Done: false,
    Completed: false,
    Rejected: false,
  });
  const [localColumns, setLocalColumns] = useState<Record<RequirementStatus, string[]>>(
    emptyColumns(),
  );
  const [lastMoved, setLastMoved] = useState<{ id: string; status: RequirementStatus } | null>(null);
  const [highlightedCardId, setHighlightedCardId] = useState<string | null>(null);
  const clearHighlightRef = useRef<number | null>(null);
  const previousDetailIdRef = useRef<string | null>(null);
  const lastAppliedSavedViewRef = useRef<string | null>(null);
  const {
    views: allSavedViews,
    saveView,
    deleteView,
    getViewById,
    getDefaultView,
  } = useSavedViews();

  const savedViews = useMemo(
    () => allSavedViews.filter((view) => view.page === 'kanban'),
    [allSavedViews],
  );

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
  );

  const queryFields = useMemo(
    () => buildQueryFields(requirements ?? []),
    [requirements],
  );

  const filtered = useMemo(() => {
    if (!requirements) return [];
    const simple = applyFilters(requirements).filter((req) => selectedStatuses.includes(req.status));
    return applyAdvancedFilter(simple);
  }, [requirements, applyFilters, selectedStatuses, applyAdvancedFilter]);

  const visibleStatuses = useMemo(
    () => STATUS_ORDER.filter((status) => selectedStatuses.includes(status)),
    [selectedStatuses],
  );

  const requirementsById = useMemo(() => {
    const map = new Map<string, Requirement>();
    for (const req of filtered) {
      map.set(req.id, req);
    }
    return map;
  }, [filtered]);

  useEffect(() => {
    setLocalColumns((prev) => {
      const next = emptyColumns();

      for (const status of STATUS_ORDER) {
        const incoming = filtered
          .filter((req) => req.status === status)
          .map((req) => req.id);
        const prevOrdered = (prev[status] ?? []).filter((id) => incoming.includes(id));
        const additions = incoming.filter((id) => !prevOrdered.includes(id));
        next[status] = [...prevOrdered, ...additions];
      }

      return next;
    });
  }, [filtered]);

  const firstCardId = useMemo(() => {
    for (const status of visibleStatuses) {
      const first = localColumns[status]?.[0];
      if (first) return first;
    }
    return null;
  }, [localColumns, visibleStatuses]);

  useEffect(() => {
    const remembered =
      typeof window !== 'undefined'
        ? window.sessionStorage.getItem('kanban.selectedCardId')
        : null;
    if (selectedCardId && requirementsById.has(selectedCardId)) return;
    if (remembered && requirementsById.has(remembered)) {
      setSelectedCardId(remembered);
      return;
    }
    setSelectedCardId(firstCardId);
  }, [firstCardId, requirementsById, selectedCardId]);

  useEffect(() => {
    if (!selectedCardId || typeof window === 'undefined') return;
    window.sessionStorage.setItem('kanban.selectedCardId', selectedCardId);
  }, [selectedCardId]);

  const findContainer = useCallback(
    (id: string): RequirementStatus | null => {
      if (STATUS_ORDER.includes(id as RequirementStatus)) {
        return id as RequirementStatus;
      }
      for (const status of visibleStatuses) {
        if (localColumns[status].includes(id)) {
          return status;
        }
      }
      return null;
    },
    [localColumns, visibleStatuses],
  );

  const columnMap = useMemo(() => {
    const map: Record<RequirementStatus, Requirement[]> = {
      Draft: [], Approved: [], Planned: [], InProgress: [], NeedsAttention: [], Done: [], Completed: [], Rejected: [],
    };

    for (const status of STATUS_ORDER) {
      map[status] = localColumns[status]
        .map((id) => requirementsById.get(id))
        .filter((req): req is Requirement => !!req)
        .map((req) => (req.status === status ? req : { ...req, status }));
    }

    return map;
  }, [localColumns, requirementsById]);

  const activeReq = useMemo(
    () => (activeId ? requirementsById.get(activeId) ?? null : null),
    [requirementsById, activeId],
  );

  const handleDragStart = useCallback((event: DragStartEvent) => {
    const id = event.active.id as string;
    setActiveId(id);
    setSelectedCardId(id);
  }, []);

  const handleDragOver = useCallback(
    (event: DragOverEvent) => {
      const { active, over } = event;
      if (!over) return;

      const activeKey = active.id as string;
      const overKey = over.id as string;

      const activeContainer = findContainer(activeKey);
      const overContainer = findContainer(overKey);
      if (!activeContainer || !overContainer || activeContainer === overContainer) return;

      setLocalColumns((prev) => {
        const sourceItems = [...prev[activeContainer]];
        const targetItems = [...prev[overContainer]];
        const activeIndex = sourceItems.indexOf(activeKey);
        if (activeIndex < 0) return prev;

        sourceItems.splice(activeIndex, 1);

        let targetIndex = targetItems.length;
        if (!STATUS_ORDER.includes(overKey as RequirementStatus)) {
          const overIndex = targetItems.indexOf(overKey);
          if (overIndex >= 0) targetIndex = overIndex;
        }
        targetItems.splice(targetIndex, 0, activeKey);

        return {
          ...prev,
          [activeContainer]: sourceItems,
          [overContainer]: targetItems,
        };
      });
    },
    [findContainer],
  );

  const markMovedCard = useCallback((id: string, status: RequirementStatus) => {
    setLastMoved({ id, status });
    setHighlightedCardId(id);
    setSelectedCardId(id);
    if (clearHighlightRef.current) {
      window.clearTimeout(clearHighlightRef.current);
    }
    clearHighlightRef.current = window.setTimeout(() => {
      setHighlightedCardId(null);
    }, 1600);
  }, []);

  const handleKeyboardMove = useCallback(
    (id: string, direction: 'left' | 'right' | 'up' | 'down') => {
      const source = findContainer(id);
      if (!source) return;
      const sourceIndex = visibleStatuses.indexOf(source);
      if (sourceIndex < 0) return;

      if (direction === 'left' || direction === 'right') {
        const targetIndex = direction === 'left' ? sourceIndex - 1 : sourceIndex + 1;
        if (targetIndex < 0 || targetIndex >= visibleStatuses.length) return;
        const target = visibleStatuses[targetIndex];

        setLocalColumns((prev) => {
          const sourceItems = prev[source].filter((itemId) => itemId !== id);
          const targetItems = [id, ...prev[target].filter((itemId) => itemId !== id)];
          return {
            ...prev,
            [source]: sourceItems,
            [target]: targetItems,
          };
        });

        const req = requirementsById.get(id);
        if (req && req.status !== target) {
          updateReq.mutate({
            id: req.spec_id ?? req.id,
            data: { status: target },
          });
        }
        markMovedCard(id, target);
        return;
      }

      setLocalColumns((prev) => {
        const items = [...prev[source]];
        const idx = items.indexOf(id);
        if (idx < 0) return prev;
        const nextIdx = direction === 'up' ? idx - 1 : idx + 1;
        if (nextIdx < 0 || nextIdx >= items.length) return prev;
        return {
          ...prev,
          [source]: arrayMove(items, idx, nextIdx),
        };
      });
      markMovedCard(id, source);
    },
    [findContainer, markMovedCard, requirementsById, updateReq, visibleStatuses],
  );

  const focusCard = useCallback((id: string) => {
    const raf = window.requestAnimationFrame(() => {
      const el = document.querySelector<HTMLElement>(`[data-kanban-card-id="${id}"]`);
      if (el) {
        el.focus();
        el.scrollIntoView({ behavior: 'smooth', block: 'center', inline: 'nearest' });
      }
    });
    return () => window.cancelAnimationFrame(raf);
  }, []);

  const navigateSelection = useCallback(
    (id: string, direction: 'left' | 'right' | 'up' | 'down') => {
      const source = findContainer(id);
      if (!source) return;
      const sourceItems = localColumns[source];
      const idx = sourceItems.indexOf(id);
      if (idx < 0) return;

      let nextId: string | null = null;
      if (direction === 'up') {
        nextId = idx > 0 ? sourceItems[idx - 1] : sourceItems[idx];
      } else if (direction === 'down') {
        nextId = idx < sourceItems.length - 1 ? sourceItems[idx + 1] : sourceItems[idx];
      } else {
        const sourceIndex = visibleStatuses.indexOf(source);
        const targetIndex = direction === 'left' ? sourceIndex - 1 : sourceIndex + 1;
        if (targetIndex < 0 || targetIndex >= visibleStatuses.length) return;
        const target = visibleStatuses[targetIndex];
        const targetItems = localColumns[target];
        if (targetItems.length === 0) return;
        const clamped = Math.min(idx, targetItems.length - 1);
        nextId = targetItems[clamped];
      }

      if (nextId) {
        setSelectedCardId(nextId);
        focusCard(nextId);
      }
    },
    [findContainer, focusCard, localColumns, visibleStatuses],
  );

  const openSelectedDetail = useCallback(
    (id: string, startEditDescription: boolean) => {
      const req = requirementsById.get(id);
      if (!req) return;
      open(req.spec_id ?? req.id, {
        startInDescriptionEdit: startEditDescription,
      });
    },
    [open, requirementsById],
  );

  const handleCardActivate = useCallback(
    (id: string) => {
      setSelectedCardId(id);
      focusCard(id);
      openSelectedDetail(id, false);
    },
    [focusCard, openSelectedDetail],
  );

  const handleCardKeyAction = useCallback(
    (
      id: string,
      action: 'left' | 'right' | 'up' | 'down' | 'enter' | 'space' | 'escape',
    ) => {
      setSelectedCardId(id);
      if (action === 'enter') {
        const req = requirementsById.get(id);
        if (!req) return;
        const reqKey = req.spec_id ?? req.id;
        if (!detailId || detailId !== reqKey) {
          openSelectedDetail(id, false);
        } else if (detailMode !== 'edit-desc') {
          openSelectedDetail(id, true);
        }
        return;
      }
      if (action === 'space') {
        setMode((prev) => (prev === 'navigation' ? 'move' : 'navigation'));
        return;
      }
      if (action === 'escape') {
        setMode('navigation');
        return;
      }
      if (mode === 'move') {
        handleKeyboardMove(id, action);
      } else {
        navigateSelection(id, action);
      }
    },
    [detailId, detailMode, handleKeyboardMove, mode, navigateSelection, openSelectedDetail, requirementsById],
  );

  const handleToggleStatus = useCallback((status: RequirementStatus) => {
    setSelectedStatuses((prev) => {
      const isSelected = prev.includes(status);
      if (!isSelected) return [...prev, status];
      if (prev.length === 1) return prev;
      return prev.filter((s) => s !== status);
    });
  }, []);

  const handleSelectAllStatuses = useCallback(() => {
    setSelectedStatuses([...STATUS_ORDER]);
  }, []);

  const handleToggleCollapse = useCallback((status: RequirementStatus) => {
    setCollapsedStatuses((prev) => ({
      ...prev,
      [status]: !prev[status],
    }));
  }, []);

  const applySavedView = useCallback(
    (viewId: string) => {
      const view = getViewById(viewId);
      if (!view || view.page !== 'kanban') return;
      lastAppliedSavedViewRef.current = view.id;
      setShowFilterBar(view.showFilterBar);
      setSelectedStatuses(view.kanbanSelectedStatuses?.length ? view.kanbanSelectedStatuses : [...STATUS_ORDER]);
      setCollapsedStatuses(view.kanbanCollapsedStatuses ?? {
        Draft: false,
        Approved: false,
        Planned: false,
        InProgress: false,
        NeedsAttention: false,
        Done: false,
        Completed: false,
        Rejected: false,
      });
      onQueryChange(view.advancedQuery ?? EMPTY_ADVANCED_QUERY);
      setSearchParams((prev) => {
        const next = new URLSearchParams(prev);
        const keysToReset = ['status', 'priority', 'type', 'feature', 'owner', 'tag'];
        for (const key of keysToReset) next.delete(key);
        const nextFilters = view.filters;
        if (nextFilters.status) next.set('status', nextFilters.status);
        if (nextFilters.priority) next.set('priority', nextFilters.priority);
        if (nextFilters.type) next.set('type', nextFilters.type);
        if (nextFilters.feature) next.set('feature', nextFilters.feature);
        if (nextFilters.owner) next.set('owner', nextFilters.owner);
        if (nextFilters.tag) next.set('tag', nextFilters.tag);
        next.set('sv', view.id);
        return next;
      });
    },
    [getViewById, onQueryChange, setSearchParams],
  );

  const handleUpdateSavedViewSettings = useCallback(
    (id: string, patch: SavedViewSettingsPatch) => {
      const existing = getViewById(id);
      if (!existing || existing.page !== 'kanban') return;
      const saved = saveView({
        id: existing.id,
        name: existing.name,
        page: existing.page,
        isDefault: patch.isDefault ?? existing.isDefault,
        showFilterBar: patch.showFilterBar ?? existing.showFilterBar,
        showInSidebar: patch.showInSidebar ?? existing.showInSidebar,
        filters: existing.filters,
        advancedQuery: existing.advancedQuery,
        kanbanSelectedStatuses: existing.kanbanSelectedStatuses,
        kanbanCollapsedStatuses: existing.kanbanCollapsedStatuses,
      });
      applySavedView(saved.id);
    },
    [applySavedView, getViewById, saveView],
  );

  const hotkeys: HotkeyBinding[] = useMemo(
    () => [
      {
        id: 'kanban:toggle-advanced-filter',
        description: 'Toggle advanced filter',
        category: 'Kanban',
        keys: ['f'],
        handler: toggleAdvanced,
      },
    ],
    [toggleAdvanced],
  );

  useHotkeys(hotkeys);

  useEffect(() => {
    const selectedSavedViewId = searchParams.get('sv');
    if (selectedSavedViewId) {
      if (lastAppliedSavedViewRef.current !== selectedSavedViewId) {
        applySavedView(selectedSavedViewId);
      }
      return;
    }
    const defaultView = getDefaultView('kanban');
    if (defaultView && lastAppliedSavedViewRef.current !== defaultView.id) {
      applySavedView(defaultView.id);
    }
  }, [applySavedView, getDefaultView, searchParams]);

  const handleDragEnd = useCallback(
    (event: DragEndEvent) => {
      setActiveId(null);
      const { active, over } = event;
      if (!over) return;

      const activeKey = active.id as string;
      const overKey = over.id as string;
      const activeContainer = findContainer(activeKey);
      const overContainer = findContainer(overKey);
      if (!activeContainer || !overContainer) return;

      if (activeContainer === overContainer) {
        if (!STATUS_ORDER.includes(overKey as RequirementStatus)) {
          setLocalColumns((prev) => {
            const items = [...prev[activeContainer]];
            const oldIndex = items.indexOf(activeKey);
            const newIndex = items.indexOf(overKey);
            if (oldIndex < 0 || newIndex < 0 || oldIndex === newIndex) return prev;
            return {
              ...prev,
              [activeContainer]: arrayMove(items, oldIndex, newIndex),
            };
          });
        }
      }

      const draggedReq = requirementsById.get(activeKey);
      if (!draggedReq) return;
      if (draggedReq.status !== overContainer) {
        updateReq.mutate({
          id: draggedReq.spec_id ?? draggedReq.id,
          data: { status: overContainer },
        });
      }

      markMovedCard(activeKey, overContainer);
    },
    [findContainer, markMovedCard, requirementsById, updateReq],
  );

  useEffect(() => {
    if (!lastMoved) return;
    return focusCard(lastMoved.id);
  }, [lastMoved, focusCard]);

  useEffect(() => {
    const wasOpen = !!previousDetailIdRef.current;
    const isOpen = !!detailId;
    if (wasOpen && !isOpen && selectedCardId) {
      focusCard(selectedCardId);
    }
    previousDetailIdRef.current = detailId;
  }, [detailId, focusCard, selectedCardId]);

  useEffect(() => {
    if (!selectedCardId) return;
    // Keep selected card in view when selection changes.
    return focusCard(selectedCardId);
  }, [selectedCardId, focusCard]);

  useEffect(() => {
    return () => {
      if (clearHighlightRef.current) {
        window.clearTimeout(clearHighlightRef.current);
      }
    };
  }, []);

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
    <div className="space-y-4 min-w-0">
      <div className="flex items-center justify-between gap-3">
        <h1 className="text-xl font-semibold text-content">Kanban Board</h1>
        <div className="flex items-center gap-2">
          <button
            onClick={() => setShowFilterBar((prev) => !prev)}
            className={cn(
              'rounded-lg border px-2.5 py-1.5 text-xs font-medium transition-colors',
              showFilterBar
                ? 'border-accent/40 bg-accent/10 text-accent'
                : 'border-edge text-content-muted hover:text-content hover:bg-surface-hover',
            )}
            title="Show or hide basic filters"
          >
            {showFilterBar ? 'Hide Filters' : 'Show Filters'}
          </button>
          <SavedViewsPicker
            page="kanban"
            views={savedViews}
            onLoad={(view) => applySavedView(view.id)}
            onDelete={deleteView}
            onUpdateSettings={handleUpdateSavedViewSettings}
          />
          <button
            onClick={toggleAdvanced}
            className={cn(
              'flex items-center gap-1.5 rounded-lg border px-2.5 py-1.5 text-xs font-medium transition-colors',
              advancedOpen || hasActiveQuery
                ? 'border-accent bg-accent/10 text-accent'
                : 'border-edge text-content-muted hover:text-content hover:bg-surface-hover',
            )}
            title="Toggle advanced query builder"
          >
            <SlidersHorizontal className="h-3.5 w-3.5" />
            Advanced
            {hasActiveQuery && (
              <span className="rounded-full bg-accent px-1.5 text-[10px] font-bold text-white">
                ON
              </span>
            )}
          </button>
          <span
            className="rounded-full bg-surface-hover px-2.5 py-1 text-[11px] text-content-secondary"
            title="Space toggles mode"
          >
            Mode: {mode === 'move' ? 'Move' : 'Navigation'}
          </span>
          <span
            className="rounded-full border border-edge bg-surface px-2.5 py-1 text-[11px] text-content-muted"
            title={mode === 'move'
              ? 'Move mode: arrows reorder or change status'
              : 'Navigation mode: arrows change selected card'}
          >
            {mode === 'move'
              ? 'Arrows: Move | Space/Esc: Exit'
              : 'Arrows: Select | Enter: View/Edit | Space: Move'}
          </span>
        </div>
      </div>
      <p className="text-xs text-content-muted">
        Tip: Navigation mode uses arrows to select cards. Enter opens details (press Enter again to edit description), Space toggles Move mode, Esc exits Move mode.
      </p>

      {showFilterBar && (
        <KanbanFilterBar
          requirements={requirements ?? []}
          selectedStatuses={selectedStatuses}
          onToggleStatus={handleToggleStatus}
          onSelectAllStatuses={handleSelectAllStatuses}
        />
      )}

      {advancedOpen && (
        <AdvancedQueryBuilder
          query={advancedQuery}
          onQueryChange={onQueryChange}
          fields={queryFields}
          onClear={clearQuery}
          hasActiveQuery={hasActiveQuery}
          savedQueries={savedQueries}
          onSaveQuery={saveQuery}
          onLoadQuery={loadSavedQuery}
          onDeleteQuery={deleteSavedQuery}
        />
      )}

      {filtered.length === 0 ? (
        <EmptyState
          icon={<Columns3 className="h-10 w-10" />}
          title="No requirements found"
          description="Try adjusting your filters or add new requirements."
        />
      ) : (
        <DndContext
          sensors={sensors}
          collisionDetection={pointerWithin}
          onDragStart={handleDragStart}
          onDragOver={handleDragOver}
          onDragEnd={handleDragEnd}
        >
          <div
            className="max-w-full overflow-x-auto overflow-y-hidden pb-4"
            style={{ scrollbarGutter: 'stable both-edges' }}
          >
            <div className="flex min-w-max gap-4 pr-2">
              {visibleStatuses.map((status) => (
                <KanbanColumn
                  key={status}
                  status={status}
                  requirements={columnMap[status]}
                  highlightedReqId={
                    lastMoved?.status === status ? highlightedCardId : null
                  }
                  selectedReqId={selectedCardId}
                  collapsed={collapsedStatuses[status]}
                  onToggleCollapse={handleToggleCollapse}
                  onCardActivate={handleCardActivate}
                  onCardKeyAction={handleCardKeyAction}
                />
              ))}
            </div>
          </div>

          <DragOverlay>
            {activeReq ? <KanbanCard requirement={activeReq} isDragOverlay /> : null}
          </DragOverlay>
        </DndContext>
      )}
    </div>
  );
}
