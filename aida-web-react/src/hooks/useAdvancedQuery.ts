import { useState, useCallback, useMemo, useEffect } from 'react';
import { useSearchParams } from 'react-router-dom';
import type { RuleGroupType } from 'react-querybuilder';
import type { Requirement } from '@shared/types';
import { evaluateAdvancedQuery } from '../lib/query-eval';

const STORAGE_KEY = 'aida-saved-queries';

export interface SavedQuery {
  name: string;
  query: RuleGroupType;
  createdAt: string;
}

const DEFAULT_QUERY: RuleGroupType = { combinator: 'and', rules: [] };

function encodeQuery(query: RuleGroupType): string {
  try {
    return btoa(JSON.stringify(query));
  } catch {
    return '';
  }
}

function decodeQuery(encoded: string): RuleGroupType | null {
  try {
    return JSON.parse(atob(encoded)) as RuleGroupType;
  } catch {
    return null;
  }
}

function loadSavedQueries(): SavedQuery[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    return JSON.parse(raw) as SavedQuery[];
  } catch {
    return [];
  }
}

function persistSavedQueries(queries: SavedQuery[]): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(queries));
}

export function useAdvancedQuery() {
  const [searchParams, setSearchParams] = useSearchParams();

  // Initialize query from URL param if present
  const initialQuery = useMemo(() => {
    const aq = searchParams.get('aq');
    if (aq) {
      const decoded = decodeQuery(aq);
      if (decoded) return decoded;
    }
    return DEFAULT_QUERY;
    // Only run on mount
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const [query, setQuery] = useState<RuleGroupType>(initialQuery);
  const [isOpen, setIsOpen] = useState(false);
  const [savedQueries, setSavedQueries] = useState<SavedQuery[]>(loadSavedQueries);

  // Sync query to URL param
  useEffect(() => {
    const hasRules = query.rules.length > 0;
    setSearchParams((prev) => {
      if (hasRules) {
        prev.set('aq', encodeQuery(query));
      } else {
        prev.delete('aq');
      }
      return prev;
    }, { replace: true });
  }, [query, setSearchParams]);

  const onQueryChange = useCallback((q: RuleGroupType) => {
    setQuery(q);
  }, []);

  const clearQuery = useCallback(() => {
    setQuery(DEFAULT_QUERY);
  }, []);

  const toggleOpen = useCallback(() => {
    setIsOpen((prev) => !prev);
  }, []);

  /** Filter requirements through the advanced query. No-op when no rules. */
  const applyAdvancedFilter = useCallback(
    (requirements: Requirement[]): Requirement[] => {
      if (query.rules.length === 0) return requirements;
      return evaluateAdvancedQuery(query, requirements);
    },
    [query],
  );

  const saveQuery = useCallback(
    (name: string) => {
      const entry: SavedQuery = { name, query, createdAt: new Date().toISOString() };
      setSavedQueries((prev) => {
        // Replace if same name exists
        const next = prev.filter((q) => q.name !== name);
        next.push(entry);
        persistSavedQueries(next);
        return next;
      });
    },
    [query],
  );

  const loadSavedQuery = useCallback((name: string) => {
    const queries = loadSavedQueries();
    const found = queries.find((q) => q.name === name);
    if (found) {
      setQuery(found.query);
      setIsOpen(true);
    }
  }, []);

  const deleteSavedQuery = useCallback((name: string) => {
    setSavedQueries((prev) => {
      const next = prev.filter((q) => q.name !== name);
      persistSavedQueries(next);
      return next;
    });
  }, []);

  const hasActiveQuery = query.rules.length > 0;

  return {
    query,
    onQueryChange,
    clearQuery,
    isOpen,
    setIsOpen,
    toggleOpen,
    applyAdvancedFilter,
    hasActiveQuery,
    savedQueries,
    saveQuery,
    loadSavedQuery,
    deleteSavedQuery,
  };
}
