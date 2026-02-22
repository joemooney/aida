// trace:STORY-0375 | ai:claude
import { useState, useMemo, useCallback } from 'react';
import { useHotkeys, type HotkeyBinding } from './useHotkeys';
import { useDetailPanel } from './useDetailPanel';
import { useAddToQueue } from './useQueue';

export function useListSelection(itemIds: string[]) {
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const { detailId, open } = useDetailPanel();
  const addToQueue = useAddToQueue();

  const selectNext = useCallback(() => {
    if (itemIds.length === 0) return;
    if (selectedId === null) {
      setSelectedId(itemIds[0]);
    } else {
      const idx = itemIds.indexOf(selectedId);
      if (idx < itemIds.length - 1) {
        setSelectedId(itemIds[idx + 1]);
      }
    }
  }, [itemIds, selectedId]);

  const selectPrev = useCallback(() => {
    if (itemIds.length === 0) return;
    if (selectedId === null) {
      setSelectedId(itemIds[itemIds.length - 1]);
    } else {
      const idx = itemIds.indexOf(selectedId);
      if (idx > 0) {
        setSelectedId(itemIds[idx - 1]);
      }
    }
  }, [itemIds, selectedId]);

  const openSelected = useCallback(() => {
    if (selectedId) open(selectedId);
  }, [selectedId, open]);

  const queueSelected = useCallback(() => {
    if (selectedId) addToQueue.mutate({ requirement_id: selectedId });
  }, [selectedId, addToQueue]);

  const clearSelection = useCallback(() => {
    if (selectedId && !detailId) {
      setSelectedId(null);
    }
  }, [selectedId, detailId]);

  const bindings: HotkeyBinding[] = useMemo(
    () => [
      {
        id: 'list:next',
        description: 'Select next row',
        category: 'List View',
        keys: ['j'],
        handler: selectNext,
      },
      {
        id: 'list:next-arrow',
        description: 'Select next row',
        category: 'List View',
        keys: ['ArrowDown'],
        handler: selectNext,
        enabled: selectedId !== null,
      },
      {
        id: 'list:prev',
        description: 'Select previous row',
        category: 'List View',
        keys: ['k'],
        handler: selectPrev,
      },
      {
        id: 'list:prev-arrow',
        description: 'Select previous row',
        category: 'List View',
        keys: ['ArrowUp'],
        handler: selectPrev,
        enabled: selectedId !== null,
      },
      {
        id: 'list:open',
        description: 'Open detail panel',
        category: 'List View',
        keys: ['Enter'],
        handler: openSelected,
        enabled: selectedId !== null,
      },
      {
        id: 'list:queue',
        description: 'Add to queue',
        category: 'List View',
        keys: ['q'],
        handler: queueSelected,
        enabled: selectedId !== null,
      },
      {
        id: 'list:clear',
        description: 'Clear selection',
        category: 'List View',
        keys: ['Escape'],
        ignoreInInput: false,
        handler: clearSelection,
        enabled: selectedId !== null && !detailId,
      },
    ],
    [selectNext, selectPrev, openSelected, queueSelected, clearSelection, selectedId, detailId],
  );

  useHotkeys(bindings, [selectNext, selectPrev, openSelected, queueSelected, clearSelection, selectedId, detailId]);

  return { selectedId, setSelectedId };
}
