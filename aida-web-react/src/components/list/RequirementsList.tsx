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
  useDroppable,
  type DragStartEvent,
  type DragEndEvent,
} from '@dnd-kit/core';
import { ArrowUpDown, List, GitBranch, ChevronsDownUp, ChevronsUpDown, ListPlus, XCircle, SlidersHorizontal } from 'lucide-react';
import type { Requirement } from '@shared/types';
import { cn } from '../../lib/utils';
import { useRequirements, useSetParent, useUpdateRequirement } from '../../hooks/useRequirements';
import { useFilters } from '../../hooks/useFilters';
import { useAdvancedQuery } from '../../hooks/useAdvancedQuery';
import { useAddToQueue } from '../../hooks/useQueue';
import { useListSelection } from '../../hooks/useListSelection';
import { useHotkeys, type HotkeyBinding } from '../../hooks/useHotkeys';
import { buildQueryFields } from '../../lib/query-fields';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';
import { KanbanFilterBar } from '../kanban/KanbanFilterBar';
import { AdvancedQueryBuilder } from '../filters/AdvancedQueryBuilder';
import { SavedViewsPicker, type SavedViewSettingsPatch } from '../filters/SavedViewsPicker';
import { QuickPicker } from '../ui/QuickPicker';
import { RequirementsRow } from './RequirementsRow';
import { TreeRow } from './TreeRow';
import { buildTree, flattenTree, collectParentIds, isDescendant } from '../../lib/tree-utils';
import { useSavedViews } from '../../hooks/useSavedViews';

type SortKey = 'spec_id' | 'title' | 'status' | 'priority' | 'req_type' | 'owner' | 'modified_at';
type SortDir = 'asc' | 'desc';
type ViewMode = 'flat' | 'tree';

const columns: { key: SortKey; label: string; align?: 'right' }[] = [
  { key: 'spec_id', label: 'ID' },
  { key: 'title', label: 'Title' },
  { key: 'status', label: 'Status' },
  { key: 'priority', label: 'Priority' },
  { key: 'req_type', label: 'Type' },
  { key: 'owner', label: 'Owner' },
  { key: 'modified_at', label: 'Modified', align: 'right' },
];

function sortRequirements(reqs: Requirement[], key: SortKey, dir: SortDir): Requirement[] {
  return [...reqs].sort((a, b) => {
    const aVal = String(a[key] ?? '');
    const bVal = String(b[key] ?? '');
    const cmp = aVal.localeCompare(bVal, undefined, { numeric: true });
    return dir === 'asc' ? cmp : -cmp;
  });
}

function QueueDropZone({ isActive }: { isActive: boolean }) {
  const { setNodeRef, isOver } = useDroppable({ id: 'queue-drop-zone' });

  if (!isActive) return null;

  return (
    <div
      ref={setNodeRef}
      className={cn(
        'flex items-center justify-center gap-2 rounded-lg border-2 border-dashed px-4 py-3 text-sm font-medium transition-colors',
        isOver
          ? 'border-accent bg-accent/10 text-accent'
          : 'border-edge text-content-muted',
      )}
    >
      <ListPlus className="h-4 w-4" />
      Drop here to add to My Queue
    </div>
  );
}

function RootDropZone({ isActive }: { isActive: boolean }) {
  const { setNodeRef, isOver } = useDroppable({ id: 'root-drop-zone' });

  if (!isActive) return null;

  return (
    <div
      ref={setNodeRef}
      className={cn(
        'flex items-center justify-center gap-2 rounded-lg border-2 border-dashed px-4 py-3 text-sm font-medium transition-colors',
        isOver
          ? 'border-orange-400 bg-orange-400/10 text-orange-400'
          : 'border-edge text-content-muted',
      )}
    >
      <XCircle className="h-4 w-4" />
      Drop here to make root-level (remove parent)
    </div>
  );
}

type PickerKind = 'status' | 'priority' | 'owner' | null;

const STATUS_OPTIONS = ['Draft', 'Approved', 'In-Progress', 'Completed', 'Rejected'];
const PRIORITY_OPTIONS = ['High', 'Medium', 'Low'];
const EMPTY_ADVANCED_QUERY: RuleGroupType = { combinator: 'and', rules: [] };

export function RequirementsList() {
  const { data: requirements, isLoading, error } = useRequirements();
  const [searchParams, setSearchParams] = useSearchParams();
  const { applyFilters } = useFilters();
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
  const addToQueue = useAddToQueue();
  const setParent = useSetParent();
  const updateReq = useUpdateRequirement();
  const [sortKey, setSortKey] = useState<SortKey>('spec_id');
  const [sortDir, setSortDir] = useState<SortDir>('asc');
  const [viewMode, setViewMode] = useState<ViewMode>('flat');
  const [showFilterBar, setShowFilterBar] = useState(true);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [activeId, setActiveId] = useState<string | null>(null);
  const [pickerKind, setPickerKind] = useState<PickerKind>(null);
  const selectedRowRef = useRef<HTMLTableRowElement | null>(null);
  const lastAppliedSavedViewRef = useRef<string | null>(null);
  const {
    views: allSavedViews,
    saveView,
    deleteView,
    getViewById,
    getDefaultView,
  } = useSavedViews();

  const savedViews = useMemo(
    () => allSavedViews.filter((view) => view.page === 'list'),
    [allSavedViews],
  );

  const isTree = viewMode === 'tree';

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
  );

  // Build dynamic fields for the query builder
  const queryFields = useMemo(
    () => buildQueryFields(requirements ?? []),
    [requirements],
  );

  const filtered = useMemo(
    () => {
      const simple = requirements ? applyFilters(requirements) : [];
      return applyAdvancedFilter(simple);
    },
    [requirements, applyFilters, applyAdvancedFilter],
  );

  const sorted = useMemo(
    () => sortRequirements(filtered, sortKey, sortDir),
    [filtered, sortKey, sortDir],
  );

  // Build tree from ALL requirements, with filtered IDs for ancestor context
  const filteredIdSet = useMemo(
    () => new Set(filtered.map((r) => r.id)),
    [filtered],
  );

  const hasActiveFilters = requirements ? filtered.length !== requirements.length : false;

  const { roots, ancestorIds } = useMemo(
    () => buildTree(requirements ?? [], hasActiveFilters ? filteredIdSet : undefined),
    [requirements, hasActiveFilters, filteredIdSet],
  );

  const treeRows = useMemo(
    () => flattenTree(roots, collapsed, hasActiveFilters ? filteredIdSet : undefined, ancestorIds),
    [roots, collapsed, hasActiveFilters, filteredIdSet, ancestorIds],
  );

  const parentIds = useMemo(() => collectParentIds(roots), [roots]);

  // Derive display item IDs for keyboard selection (use spec_id for detail panel compat)
  const displayItemIds = useMemo(() => {
    if (isTree) {
      return treeRows.map((row) => row.node.requirement.spec_id ?? row.node.requirement.id);
    }
    return sorted.map((req) => req.spec_id ?? req.id);
  }, [isTree, treeRows, sorted]);

  const { selectedId, setSelectedId } = useListSelection(displayItemIds);

  // Scroll selected row into view
  useEffect(() => {
    if (selectedId && selectedRowRef.current) {
      selectedRowRef.current.scrollIntoView({ block: 'nearest' });
    }
  }, [selectedId]);

  // Derive unique owners from requirements for owner picker
  const ownerOptions = useMemo(() => {
    const owners = new Set<string>();
    for (const r of requirements ?? []) {
      if (r.owner) owners.add(r.owner);
    }
    return Array.from(owners).sort();
  }, [requirements]);

  // Quick picker shortcuts (Phase 3)
  const pickerBindings: HotkeyBinding[] = useMemo(
    () => [
      {
        id: 'list:status-picker',
        description: 'Change status',
        category: 'List View',
        keys: ['s'],
        handler: () => setPickerKind('status'),
        enabled: selectedId !== null,
      },
      {
        id: 'list:priority-picker',
        description: 'Change priority',
        category: 'List View',
        keys: ['p'],
        handler: () => setPickerKind('priority'),
        enabled: selectedId !== null,
      },
      {
        id: 'list:owner-picker',
        description: 'Change owner',
        category: 'List View',
        keys: ['o'],
        handler: () => setPickerKind('owner'),
        enabled: selectedId !== null,
      },
      {
        id: 'list:toggle-advanced-filter',
        description: 'Toggle advanced filter',
        category: 'List View',
        keys: ['f'],
        handler: toggleAdvanced,
      },
    ],
    [selectedId, toggleAdvanced],
  );

  useHotkeys(pickerBindings);

  const handlePickerSelect = useCallback(
    (value: string) => {
      if (!selectedId) return;
      const field = pickerKind === 'status' ? 'status' : pickerKind === 'priority' ? 'priority' : 'owner';
      updateReq.mutate({ id: selectedId, data: { [field]: value } });
      setPickerKind(null);
    },
    [selectedId, pickerKind, updateReq],
  );

  const handleToggle = useCallback((id: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  }, []);

  const expandAll = useCallback(() => setCollapsed(new Set()), []);
  const collapseAll = useCallback(() => setCollapsed(new Set(parentIds)), [parentIds]);

  function handleSort(key: SortKey) {
    if (sortKey === key) {
      setSortDir((d) => (d === 'asc' ? 'desc' : 'asc'));
    } else {
      setSortKey(key);
      setSortDir('asc');
    }
  }

  const activeReq = useMemo(
    () => (requirements ?? []).find((r) => r.id === activeId) ?? null,
    [requirements, activeId],
  );

  const handleDragStart = useCallback((event: DragStartEvent) => {
    setActiveId(event.active.id as string);
  }, []);

  const handleDragEnd = useCallback(
    (event: DragEndEvent) => {
      setActiveId(null);
      const { active, over } = event;
      if (!over) return;

      const draggedId = active.id as string;
      const overId = over.id as string;

      if (overId === 'queue-drop-zone') {
        addToQueue.mutate({ requirement_id: draggedId });
        return;
      }

      if (overId === 'root-drop-zone') {
        setParent.mutate({ id: draggedId, parentId: null });
        return;
      }

      // Reparent: drop a requirement onto another requirement row to make the
      // dragged requirement a child of the drop target. Works in both flat and
      // tree views. trace:FR-98 | ai:claude
      if (overId !== draggedId) {
        // Prevent a circular reference (dropping a parent onto its descendant).
        if (isDescendant(roots, draggedId, overId)) return;
        setParent.mutate({ id: draggedId, parentId: overId });
      }
    },
    [addToQueue, setParent, roots],
  );

  const applySavedView = useCallback(
    (viewId: string) => {
      const view = getViewById(viewId);
      if (!view || view.page !== 'list') return;
      lastAppliedSavedViewRef.current = view.id;
      setViewMode(view.listViewMode ?? 'flat');
      setShowFilterBar(view.showFilterBar);
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
      if (!existing || existing.page !== 'list') return;
      const saved = saveView({
        id: existing.id,
        name: existing.name,
        page: existing.page,
        isDefault: patch.isDefault ?? existing.isDefault,
        showFilterBar: patch.showFilterBar ?? existing.showFilterBar,
        showInSidebar: patch.showInSidebar ?? existing.showInSidebar,
        filters: existing.filters,
        advancedQuery: existing.advancedQuery,
        listViewMode: patch.listViewMode ?? existing.listViewMode ?? 'flat',
      });
      applySavedView(saved.id);
    },
    [applySavedView, getViewById, saveView],
  );

  useEffect(() => {
    const selectedSavedViewId = searchParams.get('sv');
    if (selectedSavedViewId) {
      if (lastAppliedSavedViewRef.current !== selectedSavedViewId) {
        applySavedView(selectedSavedViewId);
      }
      return;
    }
    const defaultView = getDefaultView('list');
    if (defaultView && lastAppliedSavedViewRef.current !== defaultView.id) {
      applySavedView(defaultView.id);
    }
  }, [applySavedView, getDefaultView, searchParams]);

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

  const displayCount = isTree ? treeRows.length : sorted.length;

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <h1 className="text-xl font-semibold text-content">Requirements</h1>
          <div className="flex items-center rounded-lg border border-edge overflow-hidden">
            <button
              onClick={() => setViewMode('flat')}
              className={cn(
                'flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium transition-colors',
                !isTree
                  ? 'bg-accent text-white'
                  : 'text-content-muted hover:text-content hover:bg-surface-hover',
              )}
              title="Flat list"
            >
              <List className="h-3.5 w-3.5" />
              List
            </button>
            <button
              onClick={() => setViewMode('tree')}
              className={cn(
                'flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium transition-colors',
                isTree
                  ? 'bg-accent text-white'
                  : 'text-content-muted hover:text-content hover:bg-surface-hover',
              )}
              title="Parent/child tree"
            >
              <GitBranch className="h-3.5 w-3.5" />
              Tree
            </button>
          </div>
          {isTree && parentIds.size > 0 && (
            <div className="flex items-center gap-1">
              <button
                onClick={expandAll}
                className="flex items-center gap-1 px-2 py-1 text-xs text-content-muted hover:text-content transition-colors"
                title="Expand all"
              >
                <ChevronsUpDown className="h-3.5 w-3.5" />
              </button>
              <button
                onClick={collapseAll}
                className="flex items-center gap-1 px-2 py-1 text-xs text-content-muted hover:text-content transition-colors"
                title="Collapse all"
              >
                <ChevronsDownUp className="h-3.5 w-3.5" />
              </button>
            </div>
          )}
        </div>
        <div className="flex items-center gap-3">
          <span className="text-sm text-content-muted">{displayCount} items</span>
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
            page="list"
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
            title="Toggle advanced query builder (f)"
          >
            <SlidersHorizontal className="h-3.5 w-3.5" />
            Advanced
            {hasActiveQuery && (
              <span className="rounded-full bg-accent px-1.5 text-[10px] font-bold text-white">
                ON
              </span>
            )}
          </button>
        </div>
      </div>

      {showFilterBar && <KanbanFilterBar requirements={requirements ?? []} />}

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

      {displayCount === 0 ? (
        <EmptyState
          icon={<List className="h-10 w-10" />}
          title="No requirements found"
          description="Try adjusting your filters."
        />
      ) : (
        <DndContext
          sensors={sensors}
          collisionDetection={pointerWithin}
          onDragStart={handleDragStart}
          onDragEnd={handleDragEnd}
        >
          <QueueDropZone isActive={activeId !== null} />

          <div className="rounded-xl border border-edge bg-surface-alt overflow-hidden">
            <div className="overflow-x-auto">
              <table className="w-full">
                <thead>
                  <tr className="border-b border-edge">
                    <th className="w-8" />
                    {columns.map((col) => (
                      <th
                        key={col.key}
                        onClick={isTree ? undefined : () => handleSort(col.key)}
                        className={cn(
                          'px-4 py-3 text-xs font-medium uppercase tracking-wider text-content-muted whitespace-nowrap',
                          col.align === 'right' ? 'text-right' : 'text-left',
                          isTree ? 'cursor-default' : 'cursor-pointer hover:text-content transition-colors',
                        )}
                      >
                        <span className="inline-flex items-center gap-1">
                          {col.label}
                          {!isTree && sortKey === col.key && (
                            <ArrowUpDown className="h-3 w-3 text-accent" />
                          )}
                        </span>
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {isTree
                    ? treeRows.map((row) => {
                        const rowSpecId = row.node.requirement.spec_id ?? row.node.requirement.id;
                        const isRowSelected = selectedId === rowSpecId;
                        return (
                          <TreeRow
                            key={row.node.requirement.id}
                            ref={isRowSelected ? selectedRowRef : undefined}
                            node={row.node}
                            isCollapsed={collapsed.has(row.node.requirement.id)}
                            onToggle={handleToggle}
                            isDimmed={row.isAncestorOnly}
                            isSelected={isRowSelected}
                            onClick={() => setSelectedId(rowSpecId)}
                          />
                        );
                      })
                    : sorted.map((req) => {
                        const rowSpecId = req.spec_id ?? req.id;
                        const isRowSelected = selectedId === rowSpecId;
                        return (
                          <RequirementsRow
                            key={req.id}
                            ref={isRowSelected ? selectedRowRef : undefined}
                            requirement={req}
                            isSelected={isRowSelected}
                            onClick={() => setSelectedId(rowSpecId)}
                          />
                        );
                      })}
                </tbody>
              </table>
            </div>
          </div>

          {activeId !== null && (
            <RootDropZone isActive={true} />
          )}

          <DragOverlay>
            {activeReq ? (
              <div className="flex items-center gap-2 rounded-lg border border-accent/50 bg-surface-raised px-3 py-2 shadow-xl shadow-black/30">
                <span className="text-[11px] font-mono text-content-muted">{activeReq.spec_id}</span>
                <span className="text-sm font-medium text-content">{activeReq.title}</span>
              </div>
            ) : null}
          </DragOverlay>
        </DndContext>
      )}

      {pickerKind && selectedId && (
        <QuickPicker
          anchorRef={selectedRowRef}
          options={
            pickerKind === 'status'
              ? STATUS_OPTIONS
              : pickerKind === 'priority'
                ? PRIORITY_OPTIONS
                : ownerOptions
          }
          label={pickerKind === 'status' ? 'Status' : pickerKind === 'priority' ? 'Priority' : 'Owner'}
          onSelect={handlePickerSelect}
          onClose={() => setPickerKind(null)}
        />
      )}
    </div>
  );
}
