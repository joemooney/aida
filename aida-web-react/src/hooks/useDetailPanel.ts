import { useCallback } from 'react';
import { useSearchParams } from 'react-router-dom';

interface OpenDetailOptions {
  startInDescriptionEdit?: boolean;
}

export function useDetailPanel() {
  const [searchParams, setSearchParams] = useSearchParams();
  const detailId = searchParams.get('detail');
  const detailMode = searchParams.get('detailMode');

  const open = useCallback(
    (id: string, options?: OpenDetailOptions) => {
      setSearchParams((prev) => {
        prev.set('detail', id);
        if (options?.startInDescriptionEdit) {
          prev.set('detailMode', 'edit-desc');
        } else {
          prev.delete('detailMode');
        }
        return prev;
      });
    },
    [setSearchParams],
  );

  const close = useCallback(() => {
    setSearchParams((prev) => {
      prev.delete('detail');
      prev.delete('detailMode');
      return prev;
    });
  }, [setSearchParams]);

  const setDescriptionEdit = useCallback(
    (enabled: boolean) => {
      setSearchParams((prev) => {
        if (!prev.get('detail')) return prev;
        if (enabled) prev.set('detailMode', 'edit-desc');
        else prev.delete('detailMode');
        return prev;
      });
    },
    [setSearchParams],
  );

  return { detailId, detailMode, open, close, setDescriptionEdit };
}
