import { useQuery } from '@tanstack/react-query';
import { fetchDocs, fetchDoc } from '../api/docs';
import type { DocInfo, DocDetail } from '../api/docs';

export function useDocs() {
  return useQuery<DocInfo[]>({
    queryKey: ['docs'],
    queryFn: fetchDocs,
    staleTime: 60_000,
  });
}

export function useDoc(path: string | null) {
  return useQuery<DocDetail>({
    queryKey: ['doc', path],
    queryFn: () => fetchDoc(path!),
    enabled: !!path,
  });
}
