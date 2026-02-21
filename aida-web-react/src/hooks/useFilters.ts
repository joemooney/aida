import { useCallback, useMemo } from 'react';
import { useSearchParams } from 'react-router-dom';
import type { Requirement, RequirementStatus, RequirementPriority, RequirementType } from '@shared/types';

export interface Filters {
  status: RequirementStatus | '';
  priority: RequirementPriority | '';
  type: RequirementType | '';
  feature: string;
  owner: string;
  tag: string;
}

export function useFilters() {
  const [searchParams, setSearchParams] = useSearchParams();

  const filters: Filters = useMemo(() => ({
    status: (searchParams.get('status') ?? '') as Filters['status'],
    priority: (searchParams.get('priority') ?? '') as Filters['priority'],
    type: (searchParams.get('type') ?? '') as Filters['type'],
    feature: searchParams.get('feature') ?? '',
    owner: searchParams.get('owner') ?? '',
    tag: searchParams.get('tag') ?? '',
  }), [searchParams]);

  const setFilter = useCallback(
    (key: keyof Filters, value: string) => {
      setSearchParams((prev) => {
        if (value) {
          prev.set(key, value);
        } else {
          prev.delete(key);
        }
        return prev;
      });
    },
    [setSearchParams],
  );

  const removeFilter = useCallback(
    (key: keyof Filters) => {
      setSearchParams((prev) => {
        prev.delete(key);
        return prev;
      });
    },
    [setSearchParams],
  );

  const clearFilters = useCallback(() => {
    setSearchParams((prev) => {
      prev.delete('status');
      prev.delete('priority');
      prev.delete('type');
      prev.delete('feature');
      prev.delete('owner');
      prev.delete('tag');
      return prev;
    });
  }, [setSearchParams]);

  const applyFilters = useCallback(
    (requirements: Requirement[]) => {
      return requirements.filter((req) => {
        if (filters.status && req.status !== filters.status) return false;
        if (filters.priority && req.priority !== filters.priority) return false;
        if (filters.type && req.req_type !== filters.type) return false;
        if (filters.feature && req.feature !== filters.feature) return false;
        if (filters.owner && req.owner !== filters.owner) return false;
        if (filters.tag && !(req.tags ?? []).includes(filters.tag)) return false;
        return true;
      });
    },
    [filters],
  );

  const activeFilterCount = useMemo(
    () => Object.values(filters).filter(Boolean).length,
    [filters],
  );

  return { filters, setFilter, removeFilter, clearFilters, applyFilters, activeFilterCount };
}
