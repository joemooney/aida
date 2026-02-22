// trace:STORY-0375 | ai:claude
import { useState, useRef } from 'react';
import { useHotkeys, type HotkeyBinding } from './useHotkeys';
import { useDetailPanel } from './useDetailPanel';
import { useAddToQueue } from './useQueue';

export function useListSelection(itemIds: string[]) {
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const { detailId, open } = useDetailPanel();
  const addToQueue = useAddToQueue();

  // Keep mutable refs so handlers always read fresh values
  const selectedIdRef = useRef(selectedId);
  selectedIdRef.current = selectedId;
  const itemIdsRef = useRef(itemIds);
  itemIdsRef.current = itemIds;
  const detailIdRef = useRef(detailId);
  detailIdRef.current = detailId;

  // Bindings are recreated each render but read via ref — no re-render cost
  const bindings: HotkeyBinding[] = [
    {
      id: 'list:next',
      description: 'Select next row',
      category: 'List View',
      keys: ['j'],
      handler: () => {
        const ids = itemIdsRef.current;
        const sel = selectedIdRef.current;
        if (ids.length === 0) return;
        if (sel === null) {
          setSelectedId(ids[0]);
        } else {
          const idx = ids.indexOf(sel);
          if (idx < ids.length - 1) setSelectedId(ids[idx + 1]);
        }
      },
    },
    {
      id: 'list:next-arrow',
      description: 'Select next row',
      category: 'List View',
      keys: ['ArrowDown'],
      handler: () => {
        const ids = itemIdsRef.current;
        const sel = selectedIdRef.current;
        if (ids.length === 0) return;
        if (sel === null) {
          setSelectedId(ids[0]);
        } else {
          const idx = ids.indexOf(sel);
          if (idx < ids.length - 1) setSelectedId(ids[idx + 1]);
        }
      },
      enabled: selectedId !== null,
    },
    {
      id: 'list:prev',
      description: 'Select previous row',
      category: 'List View',
      keys: ['k'],
      handler: () => {
        const ids = itemIdsRef.current;
        const sel = selectedIdRef.current;
        if (ids.length === 0) return;
        if (sel === null) {
          setSelectedId(ids[ids.length - 1]);
        } else {
          const idx = ids.indexOf(sel);
          if (idx > 0) setSelectedId(ids[idx - 1]);
        }
      },
    },
    {
      id: 'list:prev-arrow',
      description: 'Select previous row',
      category: 'List View',
      keys: ['ArrowUp'],
      handler: () => {
        const ids = itemIdsRef.current;
        const sel = selectedIdRef.current;
        if (ids.length === 0) return;
        if (sel === null) {
          setSelectedId(ids[ids.length - 1]);
        } else {
          const idx = ids.indexOf(sel);
          if (idx > 0) setSelectedId(ids[idx - 1]);
        }
      },
      enabled: selectedId !== null,
    },
    {
      id: 'list:open',
      description: 'Open detail panel',
      category: 'List View',
      keys: ['Enter'],
      handler: () => {
        const sel = selectedIdRef.current;
        if (sel) open(sel);
      },
      enabled: selectedId !== null,
    },
    {
      id: 'list:queue',
      description: 'Add to queue',
      category: 'List View',
      keys: ['q'],
      handler: () => {
        const sel = selectedIdRef.current;
        if (sel) addToQueue.mutate({ requirement_id: sel });
      },
      enabled: selectedId !== null,
    },
    {
      id: 'list:clear',
      description: 'Clear selection',
      category: 'List View',
      keys: ['Escape'],
      ignoreInInput: false,
      handler: () => {
        if (selectedIdRef.current && !detailIdRef.current) {
          setSelectedId(null);
        }
      },
      enabled: selectedId !== null && !detailId,
    },
  ];

  useHotkeys(bindings);

  return { selectedId, setSelectedId };
}
