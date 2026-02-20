import { useState, useEffect } from 'react';
import { useQuery } from '@tanstack/react-query';
import type { Requirement } from '@shared/types';
import { searchRequirements } from '../api/requirements';

function useDebounce<T>(value: T, delay: number): T {
  const [debounced, setDebounced] = useState(value);

  useEffect(() => {
    const timer = setTimeout(() => setDebounced(value), delay);
    return () => clearTimeout(timer);
  }, [value, delay]);

  return debounced;
}

export function useSearch(query: string) {
  const debouncedQuery = useDebounce(query.trim(), 250);

  return useQuery<Requirement[]>({
    queryKey: ['search', debouncedQuery],
    queryFn: () => searchRequirements(debouncedQuery),
    enabled: debouncedQuery.length > 0,
    staleTime: 60_000,
  });
}
