import { useCallback } from 'react';
import { useSearchParams } from 'react-router-dom';

export function useDetailPanel() {
  const [searchParams, setSearchParams] = useSearchParams();
  const detailId = searchParams.get('detail');

  const open = useCallback(
    (id: string) => {
      setSearchParams((prev) => {
        prev.set('detail', id);
        return prev;
      });
    },
    [setSearchParams],
  );

  const close = useCallback(() => {
    setSearchParams((prev) => {
      prev.delete('detail');
      return prev;
    });
  }, [setSearchParams]);

  return { detailId, open, close };
}
