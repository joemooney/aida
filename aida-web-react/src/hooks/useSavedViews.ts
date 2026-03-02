import { useCallback, useEffect, useMemo, useState } from 'react';
import type { RuleGroupType } from 'react-querybuilder';
import type {
  RequirementPriority,
  RequirementStatus,
  RequirementType,
} from '@shared/types';
import { useAuth } from './useAuth';

const STORAGE_KEY = 'aida.savedViews.v1';
const CHANGED_EVENT = 'aida:saved-views-changed';

export type SavedViewPage = 'list' | 'kanban';
export type ListViewMode = 'flat' | 'tree';

export interface SavedViewFilters {
  status: RequirementStatus | '';
  priority: RequirementPriority | '';
  type: RequirementType | '';
  feature: string;
  owner: string;
  tag: string;
}

export interface SavedView {
  id: string;
  name: string;
  page: SavedViewPage;
  ownerKey: string;
  createdAt: string;
  updatedAt: string;
  isDefault: boolean;
  showFilterBar: boolean;
  showInSidebar: boolean;
  filters: SavedViewFilters;
  advancedQuery: RuleGroupType | null;
  listViewMode?: ListViewMode;
  kanbanSelectedStatuses?: RequirementStatus[];
  kanbanCollapsedStatuses?: Record<RequirementStatus, boolean>;
}

export interface SaveSavedViewInput {
  id?: string;
  name: string;
  page: SavedViewPage;
  isDefault: boolean;
  showFilterBar: boolean;
  showInSidebar: boolean;
  filters: SavedViewFilters;
  advancedQuery: RuleGroupType | null;
  listViewMode?: ListViewMode;
  kanbanSelectedStatuses?: RequirementStatus[];
  kanbanCollapsedStatuses?: Record<RequirementStatus, boolean>;
}

const EMPTY_FILTERS: SavedViewFilters = {
  status: '',
  priority: '',
  type: '',
  feature: '',
  owner: '',
  tag: '',
};

function readStorage(): SavedView[] {
  if (typeof window === 'undefined') return [];
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((item): item is SavedView => !!item && typeof item === 'object');
  } catch {
    return [];
  }
}

function writeStorage(views: SavedView[]): void {
  if (typeof window === 'undefined') return;
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(views));
    window.dispatchEvent(new Event(CHANGED_EVENT));
  } catch {
    // Ignore localStorage write failures.
  }
}

function normalizeFilters(filters?: Partial<SavedViewFilters>): SavedViewFilters {
  return {
    status: filters?.status ?? EMPTY_FILTERS.status,
    priority: filters?.priority ?? EMPTY_FILTERS.priority,
    type: filters?.type ?? EMPTY_FILTERS.type,
    feature: filters?.feature ?? EMPTY_FILTERS.feature,
    owner: filters?.owner ?? EMPTY_FILTERS.owner,
    tag: filters?.tag ?? EMPTY_FILTERS.tag,
  };
}

export function useSavedViews() {
  const { user } = useAuth();
  const ownerKey = user?.handle ?? 'default';
  const [allViews, setAllViews] = useState<SavedView[]>(readStorage);

  useEffect(() => {
    function handleStorage(e: StorageEvent) {
      if (e.key && e.key !== STORAGE_KEY) return;
      setAllViews(readStorage());
    }

    function handleChanged() {
      setAllViews(readStorage());
    }

    window.addEventListener('storage', handleStorage);
    window.addEventListener(CHANGED_EVENT, handleChanged);
    return () => {
      window.removeEventListener('storage', handleStorage);
      window.removeEventListener(CHANGED_EVENT, handleChanged);
    };
  }, []);

  const userViews = useMemo(
    () => allViews.filter((v) => v.ownerKey === ownerKey),
    [allViews, ownerKey],
  );

  const replaceAllViews = useCallback((updater: (current: SavedView[]) => SavedView[]) => {
    setAllViews((current) => {
      const next = updater(current);
      writeStorage(next);
      return next;
    });
  }, []);

  const saveView = useCallback(
    (input: SaveSavedViewInput): SavedView => {
      const now = new Date().toISOString();
      const id = input.id ?? `sv_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`;

      const nextView: SavedView = {
        id,
        name: input.name.trim(),
        page: input.page,
        ownerKey,
        createdAt: now,
        updatedAt: now,
        isDefault: input.isDefault,
        showFilterBar: input.showFilterBar,
        showInSidebar: input.showInSidebar,
        filters: normalizeFilters(input.filters),
        advancedQuery: input.advancedQuery,
        listViewMode: input.listViewMode,
        kanbanSelectedStatuses: input.kanbanSelectedStatuses,
        kanbanCollapsedStatuses: input.kanbanCollapsedStatuses,
      };

      replaceAllViews((current) => {
        const existing = current.find((v) => v.id === id && v.ownerKey === ownerKey);
        const merged = existing
          ? { ...existing, ...nextView, createdAt: existing.createdAt, updatedAt: now }
          : nextView;

        const withoutThis = current.filter((v) => !(v.id === id && v.ownerKey === ownerKey));
        const withDefaultCleared = input.isDefault
          ? withoutThis.map((v) =>
              v.ownerKey === ownerKey && v.page === input.page
                ? { ...v, isDefault: false }
                : v,
            )
          : withoutThis;
        return [...withDefaultCleared, merged];
      });

      return nextView;
    },
    [ownerKey, replaceAllViews],
  );

  const deleteView = useCallback(
    (id: string) => {
      replaceAllViews((current) =>
        current.filter((v) => !(v.ownerKey === ownerKey && v.id === id)),
      );
    },
    [ownerKey, replaceAllViews],
  );

  const setDefaultView = useCallback(
    (id: string, isDefault: boolean) => {
      replaceAllViews((current) => {
        const target = current.find((v) => v.ownerKey === ownerKey && v.id === id);
        if (!target) return current;
        return current.map((v) => {
          if (v.ownerKey !== ownerKey || v.page !== target.page) return v;
          if (v.id === id) return { ...v, isDefault, updatedAt: new Date().toISOString() };
          return isDefault ? { ...v, isDefault: false } : v;
        });
      });
    },
    [ownerKey, replaceAllViews],
  );

  const setSidebarPinned = useCallback(
    (id: string, showInSidebar: boolean) => {
      replaceAllViews((current) =>
        current.map((v) =>
          v.ownerKey === ownerKey && v.id === id
            ? { ...v, showInSidebar, updatedAt: new Date().toISOString() }
            : v,
        ),
      );
    },
    [ownerKey, replaceAllViews],
  );

  const getViewById = useCallback(
    (id: string): SavedView | null => userViews.find((v) => v.id === id) ?? null,
    [userViews],
  );

  const getDefaultView = useCallback(
    (page: SavedViewPage): SavedView | null =>
      userViews
        .filter((v) => v.page === page && v.isDefault)
        .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))[0] ?? null,
    [userViews],
  );

  return {
    views: userViews,
    saveView,
    deleteView,
    setDefaultView,
    setSidebarPinned,
    getViewById,
    getDefaultView,
  };
}

